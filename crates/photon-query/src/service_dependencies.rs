//! `service_dependencies`: for one service, split its CLIENT spans into the two kinds of
//! downstream call the APM "Dependencies" view shows, and aggregate count, error count, and
//! duration percentiles (p50/p95/p99, nanoseconds) per dependency:
//!
//! - **Database** dependencies — CLIENT spans carrying a `db.system` attribute (keyed by
//!   `db.system`, or `db.system + "." + db.name` when `db.name` is present).
//! - **External** dependencies — remaining CLIENT spans that name a peer (first present of
//!   `server.address` / `net.peer.name` / `peer.service`).
//!
//! The grouping keys live in the `Map<Utf8,Utf8>` `attributes` column, not promoted columns, so
//! v1 is deliberately *scan-then-group-in-Rust*: filter to CLIENT spans (`kind == 3`), project
//! each surviving span's grouping attributes (via the same `get_field` map access the spans
//! grammar/facet uses — `span_predicate::span_field_col` on a `MapAttr`) plus `duration_nanos`
//! and `status_code`, `collect()`, then fold into two `HashMap`s. This sidesteps any DataFusion
//! map-`GROUP BY` limitation. Per-service CLIENT-span volume over a window is naturally bounded;
//! a `MAX_DEP_SPANS` scan cap (logged, never silent) guards a pathological window.
//!
//! Rate / error-rate are NOT computed here — they are DERIVED in the API handler from `count` and
//! the query window, exactly like `crate::red` (the engine stays window-agnostic beyond pruning).
use std::collections::HashMap;

use arrow::array::{Array, Int32Array, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::{lit, Expr};

use photon_core::query::SpanFieldRef;
use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::span_predicate::span_field_col;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// One downstream dependency: identity + raw counts + duration percentiles (nanoseconds). Rate
/// and error-rate are DERIVED by the API handler from `count`/`error_count` and the window.
#[derive(Debug, Clone, PartialEq)]
pub struct DepRow {
    /// Display name: the `db.system` (or `db.system.db.name`) for a database, or the peer host
    /// (`server.address` / `net.peer.name` / `peer.service`) for an external call.
    pub name: String,
    /// `db.system` for a database dependency; `None` for an external one (no protocol hint is
    /// extracted in v1).
    pub system: Option<String>,
    /// CLIENT spans in this group (the rate numerator).
    pub count: u64,
    /// Spans with `status_code == 2` (OTEL ERROR) in this group.
    pub error_count: u64,
    /// p50/p95/p99 of `duration_nanos` (nanoseconds), exact nearest-rank over the group. `0` when
    /// no span in the group has a non-null duration.
    pub p50: i64,
    pub p95: i64,
    pub p99: i64,
}

/// A service's downstream dependencies, split by kind. Each list is sorted by `count` DESC
/// (ties broken by `name` ASC for determinism).
#[derive(Debug, Clone, PartialEq)]
pub struct Dependencies {
    pub database: Vec<DepRow>,
    pub external: Vec<DepRow>,
}

/// Cap on CLIENT spans scanned for one `dependencies` call. Bounds worst-case memory/CPU of the
/// Rust-side fold; a hit is logged (never silent), matching `crate::red`'s cap convention. Beyond
/// the cap, dependency counts undercount — acceptable for a v1 breadth-first view.
const MAX_DEP_SPANS: usize = 200_000;

impl SpanQueryEngine {
    /// Database + external dependencies of the single service `req` is pre-filtered to
    /// (`service.name:<svc>`). Empty lists when nothing survives pruning / matches the predicate.
    pub async fn dependencies(&self, req: SpanQueryRequest) -> Result<Dependencies, PhotonError> {
        match self.span_survivors_df(&req).await? {
            None => Ok(Dependencies {
                database: Vec::new(),
                external: Vec::new(),
            }),
            Some(df) => deps_over(df, span_base_predicate(&req)).await,
        }
    }
}

/// The grouping attribute value expression for map key `key`, projected as a Utf8 column: the
/// same `get_field(attributes, key)` access the spans grammar/facet compiles to.
fn attr_col(key: &str) -> Expr {
    span_field_col(&SpanFieldRef::MapAttr(key.to_string()))
}

/// Per-group running aggregate.
#[derive(Default)]
struct Acc {
    /// `db.system` for a database group; `None` for an external group.
    system: Option<String>,
    count: u64,
    errors: u64,
    /// Non-null durations only (nanoseconds); null-duration spans still increment `count`.
    durations: Vec<i64>,
}

impl Acc {
    fn into_row(mut self, name: String) -> DepRow {
        self.durations.sort_unstable();
        DepRow {
            name,
            system: self.system,
            count: self.count,
            error_count: self.errors,
            p50: pct(&self.durations, 0.5),
            p95: pct(&self.durations, 0.95),
            p99: pct(&self.durations, 0.99),
        }
    }
}

/// Filter to CLIENT spans matching `predicate`, project the grouping attributes + duration +
/// status, fold into database/external groups, and compute percentiles. Split out from
/// `dependencies` so the unit tests can drive it over a `MemTable` DataFrame.
async fn deps_over(df: DataFrame, predicate: Expr) -> Result<Dependencies, PhotonError> {
    // OTLP SpanKind CLIENT == 3.
    let client_only = predicate.and(col_ref(span_schema::KIND).eq(lit(3_i32)));
    let batches = df
        .filter(client_only)
        .map_err(|e| PhotonError::Query(format!("dependencies filter: {e}")))?
        .select(vec![
            attr_col("db.system").alias("db_system"),
            attr_col("db.name").alias("db_name"),
            attr_col("server.address").alias("server_address"),
            attr_col("net.peer.name").alias("net_peer_name"),
            attr_col("peer.service").alias("peer_service"),
            col_ref(span_schema::DURATION).alias("dur"),
            col_ref(span_schema::STATUS_CODE).alias("status"),
        ])
        .map_err(|e| PhotonError::Query(format!("dependencies select: {e}")))?
        .limit(0, Some(MAX_DEP_SPANS))
        .map_err(|e| PhotonError::Query(format!("dependencies limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("dependencies collect: {e}")))?;

    let mut database: HashMap<String, Acc> = HashMap::new();
    let mut external: HashMap<String, Acc> = HashMap::new();
    let mut scanned = 0usize;

    for b in &batches {
        scanned += b.num_rows();
        let db_system = str_col(b, "db_system")?;
        let db_name = str_col(b, "db_name")?;
        let server_address = str_col(b, "server_address")?;
        let net_peer_name = str_col(b, "net_peer_name")?;
        let peer_service = str_col(b, "peer_service")?;
        let dur = i64_col(b, "dur")?;
        let status = i32_col(b, "status")?;

        for i in 0..b.num_rows() {
            let duration = if dur.is_null(i) {
                None
            } else {
                Some(dur.value(i))
            };
            let is_error = !status.is_null(i) && status.value(i) == 2;

            if let Some(sys) = non_empty(db_system, i) {
                // Database: key by db.system, refined with db.name when present.
                let key = match non_empty(db_name, i) {
                    Some(name) => format!("{sys}.{name}"),
                    None => sys.to_string(),
                };
                accumulate(
                    database.entry(key).or_insert_with(|| Acc {
                        system: Some(sys.to_string()),
                        ..Default::default()
                    }),
                    duration,
                    is_error,
                );
            } else if let Some(peer) = non_empty(server_address, i)
                .or_else(|| non_empty(net_peer_name, i))
                .or_else(|| non_empty(peer_service, i))
            {
                // External: key by the first present peer identifier.
                accumulate(
                    external.entry(peer.to_string()).or_default(),
                    duration,
                    is_error,
                );
            }
            // else: a CLIENT span with neither a db.system nor any peer key — skip it.
        }
    }

    if scanned >= MAX_DEP_SPANS {
        eprintln!(
            "photon-query: service_dependencies hit the {MAX_DEP_SPANS}-CLIENT-span scan cap; \
             dependency counts may undercount (v1 cap)"
        );
    }

    Ok(Dependencies {
        database: sorted_rows(database),
        external: sorted_rows(external),
    })
}

/// Fold one span into a group.
fn accumulate(acc: &mut Acc, duration: Option<i64>, is_error: bool) {
    acc.count += 1;
    if is_error {
        acc.errors += 1;
    }
    if let Some(d) = duration {
        acc.durations.push(d);
    }
}

/// Materialize a group map into `DepRow`s, sorted by count DESC then name ASC (deterministic).
fn sorted_rows(groups: HashMap<String, Acc>) -> Vec<DepRow> {
    let mut rows: Vec<DepRow> = groups
        .into_iter()
        .map(|(name, acc)| acc.into_row(name))
        .collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    rows
}

/// The `i`-th value of `col` if it is non-null and non-empty, else `None`. Absent / empty map
/// attributes must not create a phantom `""` group.
fn non_empty(col: &StringArray, i: usize) -> Option<&str> {
    if col.is_null(i) {
        return None;
    }
    let v = col.value(i);
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Exact nearest-rank percentile over an already-sorted slice (nanoseconds). Empty → 0.
fn pct(sorted: &[i64], q: f64) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((q * sorted.len() as f64).ceil() as usize).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

fn str_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a StringArray, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| {
            PhotonError::Query(format!("dependencies column `{name}` missing or not Utf8"))
        })
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| {
            PhotonError::Query(format!("dependencies column `{name}` missing or not Int64"))
        })
}

fn i32_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int32Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .ok_or_else(|| {
            PhotonError::Query(format!("dependencies column `{name}` missing or not Int32"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    use photon_core::query::{parse, SpanFieldResolver};
    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    use crate::SpanSort;

    fn schema() -> SpanSchema {
        SpanSchema::new(&["service.name".into()])
    }

    /// A CLIENT-ish span with an explicit `kind`, arbitrary attributes, duration, and status.
    fn client_span(
        service: &str,
        kind: i32,
        attrs: &[(&str, &str)],
        dur: Option<i64>,
        status: Option<i32>,
    ) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        SpanRecord {
            trace_id: "t1".into(),
            span_id: format!("{service}-{kind}-{}", dur.unwrap_or(0)),
            name: Some("op".into()),
            kind: Some(kind),
            start_time_nanos: 1,
            duration_nanos: dur,
            status_code: status,
            attributes,
            ..Default::default()
        }
    }

    async fn df_of(records: &[SpanRecord]) -> DataFrame {
        let schema = schema();
        let mut b = SpanBatchBuilder::new(&schema);
        for r in records {
            b.append(r);
        }
        let ctx = SessionContext::new();
        ctx.register_table(
            "spans",
            Arc::new(
                MemTable::try_new(schema.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap(),
            ),
        )
        .unwrap();
        ctx.table("spans").await.unwrap()
    }

    fn req_for(query: &str) -> SpanQueryRequest {
        SpanQueryRequest {
            start_ts_nanos: 0,
            end_ts_nanos: i64::MAX,
            query: Some(
                SpanFieldResolver::new(&["service.name".to_string()])
                    .resolve(&parse(query).unwrap())
                    .unwrap(),
            ),
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        }
    }

    #[tokio::test]
    async fn splits_database_and_external_and_aggregates() {
        let records = vec![
            client_span(
                "web",
                3,
                &[("db.system", "postgresql")],
                Some(100_000_000),
                Some(1),
            ),
            client_span(
                "web",
                3,
                &[("db.system", "postgresql")],
                Some(300_000_000),
                Some(2),
            ),
            client_span(
                "web",
                3,
                &[("server.address", "api.stripe.com")],
                Some(50_000_000),
                Some(1),
            ),
            client_span(
                "web",
                2,
                &[("db.system", "postgresql")],
                Some(10_000_000),
                Some(1),
            ), // SERVER, ignored
        ];
        let deps = deps_over(
            df_of(&records).await,
            span_base_predicate(&req_for("service.name:web")),
        )
        .await
        .unwrap();

        assert_eq!(deps.database.len(), 1);
        let pg = &deps.database[0];
        assert_eq!(pg.system.as_deref(), Some("postgresql"));
        assert_eq!(pg.count, 2);
        assert_eq!(pg.error_count, 1);
        assert!(pg.p50 > 0 && pg.p50 <= pg.p99);

        assert_eq!(deps.external.len(), 1);
        assert_eq!(deps.external[0].name, "api.stripe.com");
        assert_eq!(deps.external[0].count, 1);
    }
}
