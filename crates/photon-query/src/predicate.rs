//! Compile a `ResolvedQuery` into a DataFusion predicate `Expr` for the Parquet search path.
//!
//! Semantics MUST mirror `photon_core::query::eval`. The key reconciliation: SQL is
//! three-valued (NULL), Rust booleans are two-valued. Each term's raw predicate may be NULL
//! (absent/null field); we collapse it with the `IS [NOT] TRUE` predicate — mapping NULL→false
//! for a positive term (`raw IS TRUE`) and NULL→true for a negated term (`raw IS NOT TRUE`) —
//! exactly the in-memory `base ^ negated` where `base` is false for absent fields. See
//! `term_to_expr` for why `IS [NOT] TRUE` is used rather than a literal-outcome `CASE`. This
//! equivalence is proven by `tests/grammar_consistency.rs`.

use arrow::datatypes::DataType;
use datafusion::functions::core::expr_fn::get_field;
use datafusion::logical_expr::TryCast;
use datafusion::prelude::{lit, strpos, Expr};

use photon_core::query::{Cmp, FieldRef, ResolvedKind, ResolvedQuery, ResolvedTerm};
use photon_core::schema;

use crate::col_ref;

/// AND of every term's predicate. Empty query → `lit(true)` (matches all rows).
pub fn resolved_query_to_expr(rq: &ResolvedQuery) -> Expr {
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

fn term_to_expr(term: &ResolvedTerm) -> Expr {
    let raw = raw_expr(&term.kind);
    // Collapse SQL three-valued logic to a two-valued boolean, mirroring the in-memory
    // `base ^ negated` where an absent/null field yields `base = false`:
    //   positive term → `raw IS TRUE`      (NULL/absent → false)
    //   negated  term → `raw IS NOT TRUE`  (NULL/absent → true)
    //
    // This is the exact definitional equivalent of the brief's two `CASE` forms
    // (`CASE WHEN raw THEN true  ELSE false END` ≡ `raw IS TRUE`,
    //  `CASE WHEN raw THEN false ELSE true  END` ≡ `raw IS NOT TRUE`), but expressed with
    // the `IS [NOT] TRUE` predicates instead of a literal-outcome `CASE`. The `CASE` form is
    // rewritten *unsoundly* by DataFusion 43's `SimplifyExpressions` pass — it collapses
    // `CASE WHEN raw THEN false ELSE true END` to `NOT raw`, which under three-valued logic
    // turns NULL into NULL (row dropped) instead of the required `true`. `IS [NOT] TRUE`
    // always evaluates to a non-null boolean, so it survives the optimizer and keeps the two
    // backends in agreement (verified by `tests/grammar_consistency.rs`).
    if term.negated {
        raw.is_not_true()
    } else {
        raw.is_true()
    }
}

fn raw_expr(kind: &ResolvedKind) -> Expr {
    match kind {
        ResolvedKind::Level { ranges } => {
            let col = col_ref(schema::SEVERITY_NUMBER);
            let mut acc: Option<Expr> = None;
            for (lo, hi) in ranges {
                let r = col.clone().between(lit(*lo), lit(*hi));
                acc = Some(match acc {
                    Some(a) => a.or(r),
                    None => r,
                });
            }
            acc.unwrap_or_else(|| lit(false))
        }
        ResolvedKind::Match { field, values } => {
            let list = values.iter().map(|v| lit(v.clone())).collect();
            field_col(field).in_list(list, false)
        }
        ResolvedKind::Exists { field } => field_col(field).is_not_null(),
        ResolvedKind::Compare { field, op, value } => {
            let casted = Expr::TryCast(TryCast::new(Box::new(field_col(field)), DataType::Float64));
            let v = lit(*value);
            match op {
                Cmp::Gt => casted.gt(v),
                Cmp::Ge => casted.gt_eq(v),
                Cmp::Lt => casted.lt(v),
                Cmp::Le => casted.lt_eq(v),
            }
        }
        ResolvedKind::FreeText { text } => {
            strpos(col_ref(schema::BODY), lit(text.clone())).gt(lit(0_i64))
        }
    }
}

pub(crate) fn field_col(field: &FieldRef) -> Expr {
    match field {
        FieldRef::TraceId => col_ref(schema::TRACE_ID),
        FieldRef::SpanId => col_ref(schema::SPAN_ID),
        FieldRef::SeverityText => col_ref(schema::SEVERITY_TEXT),
        FieldRef::Attr(name) => col_ref(name),
        // Long-tail: extract the value for `name` from the `attributes` Map column. Missing key
        // ⇒ NULL, which the enclosing `IS [NOT] TRUE` collapses to false/true exactly like the
        // in-memory `attributes.get(name)` ⇒ None. `get_field` is the desugaring of
        // `attributes['name']`; it returns the Utf8 value or NULL.
        FieldRef::MapAttr(name) => get_field(col_ref(schema::ATTRIBUTES), name.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use arrow::array::{Array, TimestampNanosecondArray};
    use arrow::record_batch::RecordBatch;
    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;
    use std::collections::BTreeMap;

    use photon_core::query::{parse, FieldResolver};
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;

    fn schema() -> LogSchema {
        LogSchema::new(&[
            "service.name".into(),
            "host.name".into(),
            "status_code".into(),
        ])
    }

    fn rec(
        ts: i64,
        service: &str,
        sev: Option<i32>,
        body: &str,
        attrs: &[(&str, &str)],
    ) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        LogRecord {
            timestamp_nanos: ts,
            severity_number: sev,
            body: Some(body.into()),
            attributes,
            ..Default::default()
        }
    }

    async fn selected(records: &[LogRecord], query: &str) -> Vec<i64> {
        let schema = schema();
        let mut b = RecordBatchBuilder::new(&schema);
        for r in records {
            b.append(r);
        }
        let batch: RecordBatch = b.finish().unwrap();
        let rq = FieldResolver::new(&schema.promoted)
            .resolve(&parse(query).unwrap())
            .unwrap();
        let expr = resolved_query_to_expr(&rq);

        let ctx = SessionContext::new();
        ctx.register_table(
            "logs",
            Arc::new(MemTable::try_new(schema.arrow.clone(), vec![vec![batch]]).unwrap()),
        )
        .unwrap();
        let out = ctx
            .table("logs")
            .await
            .unwrap()
            .filter(expr)
            .unwrap()
            .collect()
            .await
            .unwrap();
        let mut ts = Vec::new();
        for batch in &out {
            let col = batch
                .column_by_name("timestamp")
                .unwrap()
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
                .unwrap();
            for i in 0..col.len() {
                ts.push(col.value(i));
            }
        }
        ts.sort();
        ts
    }

    #[tokio::test]
    async fn compiles_match_compare_and_negation_with_absent_fields() {
        let records = vec![
            rec(
                1,
                "api",
                Some(18),
                "connection timeout",
                &[("host.name", "api-1"), ("status_code", "500")],
            ),
            rec(
                2,
                "web",
                Some(10),
                "ok",
                &[("host.name", "web-1"), ("status_code", "200")],
            ),
            rec(3, "api", None, "no severity", &[]), // absent host.name/status_code, null severity
        ];
        assert_eq!(selected(&records, "service:api").await, vec![1, 3]);
        assert_eq!(selected(&records, "status_code>=500").await, vec![1]);
        assert_eq!(selected(&records, "-host.name:*").await, vec![3]); // only the absent one
        assert_eq!(selected(&records, "-status_code>=500").await, vec![2, 3]); // absent counts as excluded-match
        assert_eq!(selected(&records, "level:error").await, vec![1]);
        assert_eq!(selected(&records, "\"timeout\"").await, vec![1]);
        assert_eq!(selected(&records, "").await, vec![1, 2, 3]); // empty query → all
    }
}
