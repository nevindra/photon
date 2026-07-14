//! Exhaustive check that the two compile targets of one `ResolvedQuery` select the SAME rows:
//! the in-memory evaluator (`ResolvedQuery::matches`) and the DataFusion `Expr`
//! (`resolved_query_to_expr` over a Parquet-shaped MemTable). This is the load-bearing
//! guarantee that live tail (in-memory) and search (DataFusion) mean the same thing.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{Array, TimestampNanosecondArray};
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;

use photon_core::query::{parse, FieldResolver};
use photon_core::record::{LogRecord, RecordBatchBuilder};
use photon_core::schema::LogSchema;
use photon_query::resolved_query_to_expr;

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
    sev_text: Option<&str>,
    body: Option<&str>,
    trace: Option<&str>,
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
        severity_text: sev_text.map(String::from),
        body: body.map(String::from),
        trace_id: trace.map(String::from),
        attributes,
        ..Default::default()
    }
}

/// A deliberately varied corpus: present/absent promoted attrs, numeric & non-numeric
/// values, null severity, null body, present/absent trace_id, and adversarial numeric-string
/// `status_code` values (scientific notation, leading zeros, a decimal, and an unparsable
/// leading-space value) that stress the equivalence of DataFusion's Float64 `TryCast` (via
/// arrow-cast/`lexical_core`) and the in-memory evaluator's `str::parse::<f64>()`.
fn corpus() -> Vec<LogRecord> {
    vec![
        rec(
            1,
            "api",
            Some(18),
            Some("ERROR"),
            Some("connection timeout"),
            Some("abc"),
            &[
                ("host.name", "api-1"),
                ("status_code", "500"),
                ("region", "us-east-1"),
            ],
        ),
        rec(
            2,
            "web",
            Some(10),
            Some("INFO"),
            Some("served in 12ms"),
            None,
            &[("host.name", "web-1"), ("status_code", "200")],
        ),
        rec(
            3,
            "api",
            Some(13),
            Some("WARN"),
            Some("slow query"),
            Some("def"),
            &[
                ("host.name", "api-2"),
                ("status_code", "200"),
                ("region", "eu-west-1"),
            ],
        ),
        rec(4, "worker", None, None, None, None, &[]), // all absent/null
        rec(
            5,
            "api",
            Some(21),
            Some("FATAL"),
            Some("pool timeout waiting"),
            None,
            &[("host.name", "api-1"), ("status_code", "abc")],
        ), // non-numeric status_code
        rec(
            6,
            "web",
            Some(9),
            Some("INFO"),
            Some(""),
            Some("ghi"),
            &[("host.name", "web-2"), ("status_code", "503")],
        ),
        rec(
            7,
            "api",
            Some(14),
            Some("WARN"),
            Some("scientific status"),
            None,
            &[("host.name", "api-3"), ("status_code", "5e2")],
        ), // scientific notation: 5e2 == 500.0
        rec(
            8,
            "web",
            Some(11),
            Some("INFO"),
            Some("leading zeros"),
            None,
            &[("host.name", "web-3"), ("status_code", "007")],
        ), // leading zeros: 007 == 7.0
        rec(
            9,
            "api",
            Some(16),
            Some("WARN"),
            Some("decimal boundary"),
            None,
            &[("host.name", "api-4"), ("status_code", "499.5")],
        ), // decimal exactly at the new >=499.5 threshold, just below 500
        rec(
            10,
            "worker",
            Some(12),
            Some("INFO"),
            Some("padded numeric"),
            None,
            &[("host.name", "worker-1"), ("status_code", " 500")],
        ), // leading space: must fail to parse in BOTH backends -> excluded everywhere
    ]
}

/// Every query below resolves without error against the schema above.
const QUERIES: &[&str] = &[
    "",
    "service:api",
    "service:api,web",
    "-service:api",
    "host.name:*",
    "-host.name:*",
    "host.name:api-1",
    "status_code>=500",
    "status_code<400",
    "status_code>=499.5",
    "-status_code>=500",
    "level:error",
    "level:error,warn",
    "-level:error",
    "level:info",
    "\"timeout\"",
    "timeout",
    "-timeout",
    "trace_id:abc",
    "trace_id:*",
    "severity_text:ERROR",
    "service:api -level:debug \"pool\"",
    "service:api status_code>=500",
    "region:us-east-1", // map attr equality — matches only the record(s) with that key/value
    "-region:us-east-1", // negated — absent key counts as excluded-match (true), like promoted attrs
    "region:*",          // map attr exists — matches rows where the key is present
    "region:us-east-1 level:error", // map attr AND'd with a fixed-column term
];

async fn datafusion_selection(records: &[LogRecord], query: &str) -> Vec<i64> {
    let schema = schema();
    let mut b = RecordBatchBuilder::new(&schema);
    for r in records {
        b.append(r);
    }
    let batch = b.finish().unwrap();
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

fn in_memory_selection(records: &[LogRecord], query: &str) -> Vec<i64> {
    let rq = FieldResolver::new(&schema().promoted)
        .resolve(&parse(query).unwrap())
        .unwrap();
    let mut ts: Vec<i64> = records
        .iter()
        .filter(|r| rq.matches(r))
        .map(|r| r.timestamp_nanos)
        .collect();
    ts.sort();
    ts
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
