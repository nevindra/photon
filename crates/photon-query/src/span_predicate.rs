//! Compile a `SpanResolvedQuery` into a DataFusion predicate `Expr` for the spans Parquet path.
//! Semantics MUST mirror `photon_core::query::span_eval` (proven by `tests/span_grammar_consistency.rs`).
//! The three-valued-logic reconciliation is identical to `predicate.rs`: collapse each term's raw
//! (possibly-NULL) predicate with `IS [NOT] TRUE` — never a literal-outcome `CASE`, which
//! DataFusion 43 rewrites unsoundly.

use arrow::datatypes::DataType;
use datafusion::functions::core::expr_fn::get_field;
use datafusion::logical_expr::TryCast;
use datafusion::prelude::{lit, strpos, Expr};

use photon_core::query::{
    Cmp, SpanFieldRef, SpanResolvedKind, SpanResolvedQuery, SpanResolvedTerm,
};
use photon_core::span_schema;

use crate::col_ref;

pub fn span_resolved_query_to_expr(rq: &SpanResolvedQuery) -> Expr {
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

fn term_to_expr(term: &SpanResolvedTerm) -> Expr {
    let raw = raw_expr(&term.kind);
    if term.negated {
        raw.is_not_true()
    } else {
        raw.is_true()
    }
}

fn raw_expr(kind: &SpanResolvedKind) -> Expr {
    match kind {
        SpanResolvedKind::Status { codes } => int_in(span_schema::STATUS_CODE, codes),
        SpanResolvedKind::Kind { codes } => int_in(span_schema::KIND, codes),
        SpanResolvedKind::Match { field, values } => {
            let list = values.iter().map(|v| lit(v.clone())).collect();
            span_field_col(field).in_list(list, false)
        }
        SpanResolvedKind::Exists { field } => span_field_col(field).is_not_null(),
        SpanResolvedKind::Compare { field, op, value } => {
            let casted = Expr::TryCast(TryCast::new(
                Box::new(span_field_col(field)),
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
        SpanResolvedKind::FreeText { text } => {
            strpos(col_ref(span_schema::NAME), lit(text.clone())).gt(lit(0_i64))
        }
    }
}

fn int_in(col: &str, codes: &[i32]) -> Expr {
    let list = codes.iter().map(|c| lit(*c)).collect();
    col_ref(col).in_list(list, false)
}

pub(crate) fn span_field_col(field: &SpanFieldRef) -> Expr {
    match field {
        SpanFieldRef::TraceId => col_ref(span_schema::TRACE_ID),
        SpanFieldRef::SpanId => col_ref(span_schema::SPAN_ID),
        SpanFieldRef::ParentSpanId => col_ref(span_schema::PARENT_SPAN_ID),
        SpanFieldRef::Name => col_ref(span_schema::NAME),
        SpanFieldRef::ScopeName => col_ref(span_schema::SCOPE_NAME),
        SpanFieldRef::Duration => col_ref(span_schema::DURATION),
        SpanFieldRef::Attr(name) => col_ref(name),
        SpanFieldRef::MapAttr(name) => get_field(col_ref(span_schema::ATTRIBUTES), name.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use arrow::array::{Array, StringArray};
    use arrow::record_batch::RecordBatch;
    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;
    use std::collections::BTreeMap;

    use photon_core::query::{parse, SpanFieldResolver};
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    fn schema() -> SpanSchema {
        SpanSchema::new(&["service.name".into()])
    }

    #[allow(clippy::too_many_arguments)]
    fn span(
        span_id: &str,
        service: &str,
        name: &str,
        duration_nanos: Option<i64>,
        status_code: Option<i32>,
        kind: Option<i32>,
    ) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        SpanRecord {
            trace_id: "t1".into(),
            span_id: span_id.into(),
            name: Some(name.into()),
            duration_nanos,
            status_code,
            kind,
            attributes,
            ..Default::default()
        }
    }

    async fn selected(records: &[SpanRecord], query: &str) -> Vec<String> {
        let schema = schema();
        let mut b = SpanBatchBuilder::new(&schema);
        for r in records {
            b.append(r);
        }
        let batch: RecordBatch = b.finish().unwrap();
        let rq = SpanFieldResolver::new(&schema.promoted)
            .resolve(&parse(query).unwrap())
            .unwrap();
        let expr = span_resolved_query_to_expr(&rq);

        let ctx = SessionContext::new();
        ctx.register_table(
            "spans",
            Arc::new(MemTable::try_new(schema.arrow.clone(), vec![vec![batch]]).unwrap()),
        )
        .unwrap();
        let out = ctx
            .table("spans")
            .await
            .unwrap()
            .filter(expr)
            .unwrap()
            .collect()
            .await
            .unwrap();
        let mut ids = Vec::new();
        for batch in &out {
            let col = batch
                .column_by_name("span_id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..col.len() {
                ids.push(col.value(i).to_string());
            }
        }
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn compiles_status_kind_duration_and_negation_with_absent_fields() {
        let records = vec![
            span(
                "s1",
                "checkout",
                "charge.card",
                Some(600_000_000),
                Some(2), // error
                Some(3), // client
            ),
            span(
                "s2",
                "checkout",
                "lookup",
                Some(100_000_000),
                Some(1), // ok
                Some(2), // server
            ),
            span("s3", "payments", "noop", None, None, None),
        ];
        assert_eq!(
            selected(&records, "service:checkout").await,
            vec!["s1".to_string(), "s2".to_string()]
        );
        assert_eq!(
            selected(&records, "status:error").await,
            vec!["s1".to_string()]
        );
        assert_eq!(
            selected(&records, "kind:client").await,
            vec!["s1".to_string()]
        );
        assert_eq!(
            selected(&records, "duration>=500ms").await,
            vec!["s1".to_string()]
        );
        assert_eq!(
            selected(&records, "-status:ok").await,
            vec!["s1".to_string(), "s3".to_string()]
        );
        assert_eq!(
            selected(&records, "").await,
            vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
        );
    }
}
