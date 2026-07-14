//! The metrics label-matcher grammar compiles two ways — a DataFusion filter Expr and an
//! in-memory `MetricResolvedQuery::matches(&MetricPoint)`. This test asserts both backends select
//! the identical set of rows for an adversarial corpus × query matrix. Mirrors
//! `grammar_consistency.rs`. Each point gets a unique `id` attribute so selections are comparable.

use std::collections::BTreeMap;

use arrow::array::{Array, StringArray};
use datafusion::prelude::SessionContext;

use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
use photon_core::metric_schema::{MetricSchema, ATTRIBUTES};
use photon_core::query::parser::parse;
use photon_core::query::MetricFieldResolver;
use photon_query::metric_resolved_query_to_expr;

const PROMOTED: &[&str] = &["service.name", "http.route"];

fn promoted() -> Vec<String> {
    PROMOTED.iter().map(|s| s.to_string()).collect()
}

fn mk(id: &str, attrs: &[(&str, &str)], scope: Option<&str>) -> MetricPoint {
    let mut attributes = BTreeMap::new();
    attributes.insert("id".to_string(), id.to_string());
    for (k, v) in attrs {
        attributes.insert(k.to_string(), v.to_string());
    }
    MetricPoint {
        metric_name: "m".to_string(),
        metric_type: 0,
        type_text: None,
        temporality: None,
        is_monotonic: None,
        unit: None,
        timestamp_nanos: 0,
        start_timestamp_nanos: None,
        scope_name: scope.map(|s| s.to_string()),
        value: Some(1.0),
        histogram: None,
        exp_histogram: None,
        summary: None,
        exemplars: None,
        attributes,
    }
}

// Adversarial: present/absent promoted + map attrs, absent scope, and numeric-string values that
// stress DataFusion TryCast-to-Float64 vs Rust str::parse::<f64>() (leading space, exp form,
// leading zeros, trailing junk).
fn corpus() -> Vec<MetricPoint> {
    vec![
        mk(
            "p0",
            &[
                ("service.name", "checkout"),
                ("http.route", "/pay"),
                ("http.status_code", "500"),
            ],
            Some("otel.sdk"),
        ),
        mk(
            "p1",
            &[("service.name", "cart"), ("http.status_code", "499.5")],
            None,
        ),
        mk(
            "p2",
            &[
                ("service.name", "checkout"),
                ("deployment", "prod"),
                ("http.status_code", "5e2"),
            ],
            Some("otel.sdk"),
        ),
        mk(
            "p3",
            &[("deployment", "staging"), ("http.status_code", " 500")],
            Some("custom"),
        ),
        mk(
            "p4",
            &[
                ("service.name", "db"),
                ("http.route", "/pay"),
                ("http.status_code", "007"),
            ],
            None,
        ),
        mk("p5", &[("http.route", "/health")], Some("otel.sdk")),
        mk("p6", &[("service.name", "checkout")], None),
    ]
}

const QUERIES: &[&str] = &[
    "service:checkout",
    "service:cart,checkout",
    "-service:checkout",
    "service:*",
    "-service:*",
    "http.route:/pay",
    "http.route:*",
    "deployment:prod",
    "deployment:prod,staging",
    "-deployment:prod",
    "scope:otel.sdk",
    "-scope:otel.sdk",
    "http.status_code>=500",
    "http.status_code>500",
    "http.status_code<500",
    "http.status_code<=499.5",
    "service:checkout http.route:/pay",
    "service:checkout -deployment:prod",
    "region:*",
    "-region:*",
];

async fn datafusion_selection(records: &[MetricPoint], q: &str) -> Vec<String> {
    let schema = MetricSchema::new(&promoted());
    let mut builder = MetricBatchBuilder::new(&schema);
    for r in records {
        builder.append(r);
    }
    let batch = builder.finish().unwrap();
    let ctx = SessionContext::new();
    ctx.register_batch("metrics", batch).unwrap();

    let resolver = MetricFieldResolver::new(&promoted());
    let rq = resolver.resolve(&parse(q).unwrap()).unwrap();
    let expr = metric_resolved_query_to_expr(&rq);

    let df = ctx.table("metrics").await.unwrap().filter(expr).unwrap();
    let batches = df.collect().await.unwrap();
    let mut ids = Vec::new();
    for b in &batches {
        // `id` is a long-tail attribute; read it from the attributes Map column.
        let attrs = b
            .column_by_name(ATTRIBUTES)
            .expect("attributes column")
            .as_any()
            .downcast_ref::<arrow::array::MapArray>()
            .expect("attributes is a MapArray");
        for i in 0..b.num_rows() {
            ids.push(map_get(attrs, i, "id").expect("every row has id"));
        }
    }
    ids.sort();
    ids
}

/// Read a key's value out of row `row` of an `attributes` `MapArray`. Mirrors
/// `photon-api/src/search.rs`'s attribute decode (`downcast::<MapArray>` + `entries()` +
/// `StringArray` key/value columns), except here we know the row is non-null and want a single
/// key rather than the whole map.
fn map_get(map: &arrow::array::MapArray, row: usize, key: &str) -> Option<String> {
    let entries = map.value(row);
    let keys = entries
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let vals = entries
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    (0..keys.len())
        .find(|&i| keys.value(i) == key)
        .map(|i| vals.value(i).to_string())
}

fn in_memory_selection(records: &[MetricPoint], q: &str) -> Vec<String> {
    let resolver = MetricFieldResolver::new(&promoted());
    let rq = resolver.resolve(&parse(q).unwrap()).unwrap();
    let mut ids: Vec<String> = records
        .iter()
        .filter(|r| rq.matches(r))
        .map(|r| r.attributes.get("id").unwrap().clone())
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
