//! Exhaustive check that the two compile targets of one `SpanResolvedQuery` select the SAME
//! rows: the in-memory evaluator (`SpanResolvedQuery::matches`) and the DataFusion `Expr`
//! (`span_resolved_query_to_expr` over a `SpanBatchBuilder`-shaped MemTable). This is the
//! load-bearing guarantee that spans live-tail (in-memory) and spans search (DataFusion) mean
//! the same thing. Modeled on `tests/grammar_consistency.rs` (the logs analog).

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{Array, StringArray};
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;

use photon_core::query::{parse, SpanFieldResolver};
use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
use photon_core::span_schema::SpanSchema;
use photon_query::span_resolved_query_to_expr;

fn schema() -> SpanSchema {
    SpanSchema::new(&["service.name".into(), "http.status_code".into()])
}

#[allow(clippy::too_many_arguments)]
fn span(
    trace_id: &str,
    span_id: &str,
    parent_span_id: Option<&str>,
    name: Option<&str>,
    kind: Option<i32>,
    start: i64,
    duration_nanos: Option<i64>,
    status_code: Option<i32>,
    attrs: &[(&str, &str)],
) -> SpanRecord {
    let mut attributes = BTreeMap::new();
    for (k, v) in attrs {
        attributes.insert(k.to_string(), v.to_string());
    }
    SpanRecord {
        trace_id: trace_id.to_string(),
        span_id: span_id.to_string(),
        parent_span_id: parent_span_id.map(String::from),
        name: name.map(String::from),
        kind,
        start_time_nanos: start,
        duration_nanos,
        status_code,
        attributes,
        ..Default::default()
    }
}

/// A deliberately varied corpus: present/absent promoted attrs (`service.name`,
/// `http.status_code`), null `name`/`duration_nanos`/`status_code`/`kind`, a non-numeric
/// `http.status_code` value, adversarial numeric strings (scientific notation, leading zeros, a
/// decimal boundary, and an unparsable leading-space value) that stress the equivalence of
/// DataFusion's Float64 `TryCast` and the in-memory evaluator's `str::parse::<f64>()`, varied
/// services/operations, and a mix of root (`parent_span_id: None`) and child spans. Every span
/// has a unique `span_id` — that's the identity the two backends are compared on.
fn corpus() -> Vec<SpanRecord> {
    vec![
        // Root span with a fully-populated error status, in trace t1.
        span(
            "t1",
            "s1",
            None,
            Some("charge.card"),
            Some(3), // client
            100,
            Some(600_000_000), // 600ms
            Some(2),           // error
            &[
                ("service.name", "checkout"),
                ("http.status_code", "500"),
                ("region", "us-east-1"),
            ],
        ),
        // Child of s1, same trace, ok status.
        span(
            "t1",
            "s2",
            Some("s1"),
            Some("charge.retry"),
            Some(2), // server
            110,
            Some(50_000_000), // 50ms
            Some(1),          // ok
            &[("service.name", "checkout"), ("http.status_code", "200")],
        ),
        // Root span with region attr set to exactly "us" (for region:us exact-match).
        span(
            "t2",
            "s3",
            None,
            Some("lookup"),
            Some(1), // internal
            200,
            Some(1_000_000_000), // 1s, boundary for duration<1s
            Some(1),             // ok
            &[
                ("service.name", "payments"),
                ("http.status_code", "200"),
                ("region", "us"),
            ],
        ),
        // Child of s3: everything absent/null — absent promoted attrs, null name/duration/
        // status/kind.
        span("t2", "s4", Some("s3"), None, None, 210, None, None, &[]),
        // Root span, unset status, non-numeric http.status_code value.
        span(
            "t3",
            "s5",
            None,
            Some("noop"),
            Some(0), // unspecified
            300,
            Some(10_000_000), // 10ms
            Some(0),          // unset
            &[
                ("service.name", "worker"),
                ("http.status_code", "not_a_number"),
            ],
        ),
        // Child of s5: scientific-notation http.status_code (5e2 == 500.0), boundary for
        // duration>=500ms.
        span(
            "t3",
            "s6",
            Some("s5"),
            Some("scientific"),
            Some(4), // producer
            310,
            Some(500_000_000), // 500ms, exact boundary
            Some(2),           // error
            &[("service.name", "api"), ("http.status_code", "5e2")],
        ),
        // Root span: leading-zeros http.status_code (007 == 7.0).
        span(
            "t4",
            "s7",
            None,
            Some("leadingzero"),
            Some(5), // consumer
            400,
            Some(7_000_000), // 7ms
            Some(1),         // ok
            &[("service.name", "api"), ("http.status_code", "007")],
        ),
        // Child of s7: decimal http.status_code just below the 500 threshold.
        span(
            "t4",
            "s8",
            Some("s7"),
            Some("decimal.boundary"),
            Some(2), // server
            410,
            Some(499_500_000), // 499.5ms, just under the 500ms threshold
            Some(1),           // ok
            &[("service.name", "api"), ("http.status_code", "499.5")],
        ),
        // Root span: leading-space http.status_code must fail to parse in BOTH backends.
        span(
            "t5",
            "s9",
            None,
            Some("padded"),
            Some(3), // client
            500,
            Some(120_000_000), // 120ms
            Some(2),           // error
            &[("service.name", "web"), ("http.status_code", " 500")],
        ),
        // Child of s9: name contains "charge card" as a substring (quoted free-text target).
        span(
            "t5",
            "s10",
            Some("s9"),
            Some("process charge card"),
            Some(5), // consumer
            510,
            Some(20_000_000), // 20ms
            Some(1),          // ok
            &[("service.name", "web"), ("http.status_code", "200")],
        ),
        // Root span: region attr present but a different value (region:* exists check).
        span(
            "t6",
            "s11",
            None,
            Some("region.filter"),
            Some(2), // server
            600,
            Some(100_000_000), // 100ms
            Some(1),           // ok
            &[
                ("service.name", "worker"),
                ("http.status_code", "200"),
                ("region", "us-west-2"),
            ],
        ),
        // Child of s11: name is exactly "charge" (operation:charge exact-match target).
        span(
            "t6",
            "s12",
            Some("s11"),
            Some("charge"),
            Some(3), // client
            610,
            Some(300_000_000), // 300ms
            Some(1),           // ok
            &[("service.name", "checkout"), ("http.status_code", "200")],
        ),
    ]
}

/// Every query below resolves without error against the schema above.
const QUERIES: &[&str] = &[
    "",
    "service:checkout",
    "service:a,b",
    "-service:checkout",
    "operation:charge",
    "name:*",
    "-name:*",
    "status:error",
    "status:ok,unset",
    "-status:error",
    "kind:client",
    "kind:server,client",
    "duration>=500ms",
    "duration<1s",
    "-duration>=500ms",
    "duration>=500",
    "parent_span_id:*",
    "-parent_span_id:*",
    "trace_id:t1",
    "charge",
    "-charge",
    "\"charge card\"",
    "http.status_code>=500",
    "region:us",
    "-region:us",
    "region:*",
    "service:checkout status:error duration>=100ms",
];

async fn datafusion_selection(records: &[SpanRecord], query: &str) -> Vec<String> {
    let schema = schema();
    let mut b = SpanBatchBuilder::new(&schema);
    for r in records {
        b.append(r);
    }
    let batch = b.finish().unwrap();
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

fn in_memory_selection(records: &[SpanRecord], query: &str) -> Vec<String> {
    let rq = SpanFieldResolver::new(&schema().promoted)
        .resolve(&parse(query).unwrap())
        .unwrap();
    let mut ids: Vec<String> = records
        .iter()
        .filter(|r| rq.matches(r))
        .map(|r| r.span_id.clone())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn two_backends_select_identical_rows() {
    let records = corpus();
    for q in QUERIES {
        let df = datafusion_selection(&records, q).await;
        let mem = in_memory_selection(&records, q);
        assert_eq!(
            df, mem,
            "backends disagree for query `{q}`: datafusion={df:?} in_memory={mem:?}"
        );
    }
}
