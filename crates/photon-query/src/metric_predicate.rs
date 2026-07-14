//! Compile a resolved metrics label-matcher query into a DataFusion filter `Expr`. Mirrors
//! `predicate.rs` (logs) / `span_predicate.rs` (spans): each term collapses through `IS TRUE` /
//! `IS NOT TRUE` rather than a `CASE` (DataFusion 43's `SimplifyExpressions` rewrites the `CASE`
//! form unsoundly for NULL inputs), and map attributes are read with `get_field`.

use arrow::datatypes::DataType;
use datafusion::functions::core::expr_fn::get_field;
use datafusion::logical_expr::TryCast;
use datafusion::prelude::{lit, Expr};

use photon_core::metric_schema::{ATTRIBUTES, SCOPE_NAME};
use photon_core::query::{
    Cmp, MetricFieldRef, MetricResolvedKind, MetricResolvedQuery, MetricResolvedTerm,
};

use crate::col_ref;

/// AND all terms; an empty query matches everything.
pub fn metric_resolved_query_to_expr(rq: &MetricResolvedQuery) -> Expr {
    let mut acc: Option<Expr> = None;
    for term in &rq.terms {
        let e = term_to_expr(term);
        acc = Some(match acc {
            Some(a) => a.and(e),
            None => e,
        });
    }
    acc.unwrap_or_else(|| lit(true))
}

fn term_to_expr(term: &MetricResolvedTerm) -> Expr {
    let raw = raw_expr(&term.kind);
    if term.negated {
        raw.is_not_true()
    } else {
        raw.is_true()
    }
}

fn raw_expr(kind: &MetricResolvedKind) -> Expr {
    match kind {
        MetricResolvedKind::Match { field, values } => {
            let list = values.iter().map(|v| lit(v.clone())).collect();
            metric_field_col(field).in_list(list, false)
        }
        MetricResolvedKind::Exists { field } => metric_field_col(field).is_not_null(),
        MetricResolvedKind::Compare { field, op, value } => {
            let casted = Expr::TryCast(TryCast::new(
                Box::new(metric_field_col(field)),
                DataType::Float64,
            ));
            let v = lit(*value);
            match op {
                Cmp::Gt => casted.gt(v),
                Cmp::Ge => casted.gt_eq(v),
                Cmp::Lt => casted.lt(v),
                Cmp::Le => casted.lt_eq(v),
            }
        }
    }
}

/// The column an attribute reads from. `service.name` and other promoted attrs are their own
/// Utf8 columns (`col_ref` keeps the literal dotted name); map attrs desugar to
/// `attributes['key']`, which is SQL-NULL when absent (collapsed by the enclosing `IS [NOT] TRUE`
/// exactly like `attributes.get(name) == None` in the in-memory compiler).
pub(crate) fn metric_field_col(field: &MetricFieldRef) -> Expr {
    match field {
        MetricFieldRef::ScopeName => col_ref(SCOPE_NAME),
        MetricFieldRef::Attr(name) => col_ref(name),
        MetricFieldRef::MapAttr(name) => get_field(col_ref(ATTRIBUTES), name.as_str()),
    }
}

/// Extract the single `host.name = <v>` equality a resolved metrics query pins, if any — so a
/// host-scoped query can also set `MetricRequest.host` and prune files by the skip-index host
/// range (Task 1.3), not only filter rows. Conservative on purpose: returns `None` unless the query
/// contains exactly one non-negated `host.name:<single value>` match (an OR-list, a negation, or
/// two differing host constraints leave pruning off → keep every file, never a false-negative).
pub(crate) fn metrics_host_literal(rq: &MetricResolvedQuery) -> Option<String> {
    let mut found: Option<String> = None;
    for term in &rq.terms {
        if term.negated {
            continue;
        }
        if let MetricResolvedKind::Match {
            field: MetricFieldRef::Attr(name),
            values,
        } = &term.kind
        {
            if name == "host.name" {
                // An OR-list can't pin a single host.
                if values.len() != 1 {
                    return None;
                }
                match &found {
                    None => found = Some(values[0].clone()),
                    Some(existing) if existing == &values[0] => {}
                    // Two differing host equalities can never both hold → don't prune.
                    Some(_) => return None,
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{RecordBatch, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use datafusion::prelude::SessionContext;
    use photon_core::metric_schema::{MetricSchema, SCOPE_NAME};
    use photon_core::query::parser::parse;
    use photon_core::query::{MetricFieldResolver, MetricResolvedQuery};
    use std::sync::Arc;

    fn compile(q: &str) -> Expr {
        let r = MetricFieldResolver::new(&["service.name".to_string()]);
        let rq: MetricResolvedQuery = r.resolve(&parse(q).unwrap()).unwrap();
        metric_resolved_query_to_expr(&rq)
    }

    // A tiny table with a promoted service.name column + scope_name, so filters can be exercised
    // end-to-end. (Map-attr coverage lives in the integration consistency test.)
    async fn selected(q: &str) -> Vec<String> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("service.name", DataType::Utf8, true),
            Field::new(SCOPE_NAME, DataType::Utf8, true),
            Field::new("id", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![
                    Some("checkout"),
                    Some("cart"),
                    None,
                ])),
                Arc::new(StringArray::from(vec![Some("otel"), None, Some("otel")])),
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
            ],
        )
        .unwrap();
        let ctx = SessionContext::new();
        ctx.register_batch("t", batch).unwrap();
        let df = ctx.table("t").await.unwrap().filter(compile(q)).unwrap();
        let mut ids: Vec<String> = df
            .collect()
            .await
            .unwrap()
            .iter()
            .flat_map(|b| {
                let c = b.column(2).as_any().downcast_ref::<StringArray>().unwrap();
                (0..b.num_rows())
                    .map(|i| c.value(i).to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn metrics_host_literal_extracts_single_host_equality() {
        let resolver =
            MetricFieldResolver::new(&["service.name".to_string(), "host.name".to_string()]);
        let rq = |q: &str| resolver.resolve(&parse(q).unwrap()).unwrap();

        // A single non-negated `host.name:<v>` equality is extracted.
        assert_eq!(
            super::metrics_host_literal(&rq("host.name:web-1")),
            Some("web-1".to_string())
        );
        // Extra non-host terms don't interfere.
        assert_eq!(
            super::metrics_host_literal(&rq("service:api host.name:web-1")),
            Some("web-1".to_string())
        );
        // No host term → cannot pin a host → None (pruning stays off).
        assert_eq!(super::metrics_host_literal(&rq("service:api")), None);
        // An OR-list on host can't pin a single host → None.
        assert_eq!(
            super::metrics_host_literal(&rq("host.name:web-1,web-2")),
            None
        );
        // A negated host term never pins a host → None.
        assert_eq!(super::metrics_host_literal(&rq("-host.name:web-1")), None);
    }

    #[tokio::test]
    async fn compiles_match_negate_exists() {
        assert_eq!(selected("service:checkout").await, vec!["a"]);
        assert_eq!(selected("service:cart,checkout").await, vec!["a", "b"]);
        // negation: NOT(service = db) must also select the NULL-service row (three-valued logic).
        assert_eq!(selected("-service:cart").await, vec!["a", "c"]);
        assert_eq!(selected("scope:*").await, vec!["a", "c"]);
        let _ = MetricSchema::new(&["service.name".to_string()]); // schema import smoke
    }
}
