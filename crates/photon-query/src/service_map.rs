//! `service_map`: every service-to-service edge in the window, from one scan of the spans.
//!
//! An edge exists when a span in service B has a parent span in service A. That is the only
//! definition of "A calls B" the trace data actually contains — everything else (a peer hostname,
//! a `db.system`) describes something that is not a Service. `service_dependencies` covers those
//! and stays: a database is not a Service, and `CONTEXT.md` is explicit about it.
//!
//! **Cost, stated plainly.** The parent's service is unknown until you have read the parent, so the
//! skip index cannot prune anything: this reads every span file overlapping the window, on a narrow
//! six-column projection. There is a `MAX_MAP_SPANS` cap and hitting it is logged and reported on
//! the response (`capped`), never silent — a truncated map that looks complete is worse than no map.
//!
//! **It is a self-join, executed as a hash join in Rust rather than in DataFusion.** The two are the
//! same operation; doing it here reads the spans once instead of twice, and the `(trace_id, span_id)`
//! index borrows straight out of the Arrow buffers, so the join side costs one `&str` pair per span
//! and copies nothing. This is the same scan-then-fold shape `service_dependencies` already uses.
//!
//! **Whose latency is on an edge.** The CHILD span's duration — the callee's own service time. It is
//! deliberately not the caller's CLIENT-span duration, which would also include network and queueing:
//! two different numbers, and an edge that silently mixed them per-span would be neither.
use std::collections::HashMap;

use arrow::array::{Array, Int32Array, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::Expr;

use photon_core::span_schema;
use photon_core::PhotonError;

use crate::span_engine::span_base_predicate;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// Cap on spans scanned for one `service_map` call. The join index holds one `(&str, &str) -> &str`
/// entry per span — roughly 50 bytes with hashing overhead, so this bounds it near 15 MB. A hit is
/// logged AND surfaced as `capped` on the result; beyond the cap the map undercounts edges, and an
/// edge can go missing entirely when the cap keeps a child but drops its parent.
const MAX_MAP_SPANS: usize = 300_000;

/// One directed call edge, `from` → `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEdge {
    /// The calling service (the parent span's service).
    pub from: String,
    /// The called service (the child span's service).
    pub to: String,
    /// Child spans on this edge — how many calls.
    pub count: u64,
    /// Child spans with `status_code == 2`. The callee failed; this says nothing about whether
    /// the caller handled it.
    pub error_count: u64,
    /// p50/p95/p99 of the CHILD span's `duration_nanos`, exact nearest-rank. `0` when no span on
    /// the edge has a duration.
    pub p50: i64,
    pub p95: i64,
    pub p99: i64,
}

/// One service in the map. Counts are over ALL of its spans in the window, not just the ones on
/// edges — a node's size should mean "how busy is this", not "how connected is this".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapNode {
    pub service: String,
    pub count: u64,
    pub error_count: u64,
    /// Spans with no `parent_span_id` — trace roots. A service with roots is where work enters the
    /// system, which is what a layout wants to put on the left.
    pub roots: u64,
}

/// The map: nodes and directed edges, plus an honest account of what was scanned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceMap {
    pub nodes: Vec<MapNode>,
    pub edges: Vec<MapEdge>,
    /// Spans actually read. Reported so a thin map can be told from a truncated one.
    pub scanned: usize,
    /// `true` when [`MAX_MAP_SPANS`] bit. Edges are then incomplete.
    pub capped: bool,
}

impl SpanQueryEngine {
    /// Every service-to-service edge matching `req`, optionally narrowed to those touching `focus`
    /// in either direction.
    ///
    /// **`req` must NOT be scoped to one service.** Narrowing to `focus` happens after the edges
    /// exist, because a `service.name:x` predicate would remove exactly the parent spans the join
    /// needs — you would be left with x's spans and no way to learn who called them. The API
    /// handler enforces this by keeping `service` out of the grammar query.
    pub async fn service_map(
        &self,
        req: SpanQueryRequest,
        focus: Option<&str>,
    ) -> Result<ServiceMap, PhotonError> {
        match self.span_survivors_df(&req).await? {
            None => Ok(ServiceMap::default()),
            Some(df) => map_over(df, span_base_predicate(&req), focus).await,
        }
    }
}

/// Per-edge running aggregate.
#[derive(Default)]
struct EdgeAcc {
    count: u64,
    errors: u64,
    durations: Vec<i64>,
}

/// Per-node running aggregate.
#[derive(Default)]
struct NodeAcc {
    count: u64,
    errors: u64,
    roots: u64,
}

/// Scan, index by `(trace_id, span_id)`, then resolve each span's parent. Split out from
/// `service_map` so unit tests drive it over a `MemTable` DataFrame, no pruning and no Parquet.
pub(crate) async fn map_over(
    df: DataFrame,
    predicate: Expr,
    focus: Option<&str>,
) -> Result<ServiceMap, PhotonError> {
    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("service_map filter: {e}")))?
        .select(vec![
            col_ref(span_schema::TRACE_ID).alias("trace"),
            col_ref(span_schema::SPAN_ID).alias("span"),
            col_ref(span_schema::PARENT_SPAN_ID).alias("parent"),
            col_ref("service.name").alias("service"),
            col_ref(span_schema::DURATION).alias("dur"),
            col_ref(span_schema::STATUS_CODE).alias("status"),
        ])
        .map_err(|e| PhotonError::Query(format!("service_map select: {e}")))?
        .limit(0, Some(MAX_MAP_SPANS))
        .map_err(|e| PhotonError::Query(format!("service_map limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("service_map collect: {e}")))?;

    // Columns are borrowed once and reused by both passes; `batches` owns the Arrow buffers, so
    // every `&str` below points into them and nothing is copied.
    let mut cols = Vec::with_capacity(batches.len());
    for b in &batches {
        cols.push(Cols {
            trace: str_col(b, "trace")?,
            span: str_col(b, "span")?,
            parent: str_col(b, "parent")?,
            service: str_col(b, "service")?,
            dur: i64_col(b, "dur")?,
            status: i32_col(b, "status")?,
            rows: b.num_rows(),
        });
    }

    // --- pass 1: the join index, and every node's own totals ---
    let mut owner: HashMap<(&str, &str), &str> = HashMap::new();
    let mut nodes: HashMap<&str, NodeAcc> = HashMap::new();
    let mut scanned = 0usize;
    for c in &cols {
        scanned += c.rows;
        for i in 0..c.rows {
            let Some(service) = non_empty(c.service, i) else {
                continue; // service.name is promoted+required; a null is not a real service
            };
            let node = nodes.entry(service).or_default();
            node.count += 1;
            if is_error(c.status, i) {
                node.errors += 1;
            }
            if non_empty(c.parent, i).is_none() {
                node.roots += 1;
            }
            if let (Some(t), Some(s)) = (non_empty(c.trace, i), non_empty(c.span, i)) {
                owner.insert((t, s), service);
            }
        }
    }

    // --- pass 2: resolve each span's parent into an edge ---
    let mut edges: HashMap<(&str, &str), EdgeAcc> = HashMap::new();
    for c in &cols {
        for i in 0..c.rows {
            let (Some(child_service), Some(trace), Some(parent)) = (
                non_empty(c.service, i),
                non_empty(c.trace, i),
                non_empty(c.parent, i),
            ) else {
                continue;
            };
            // A parent outside the scan (another window, or past the cap) is not an edge we can
            // honestly draw — we do not know whose it was.
            let Some(&parent_service) = owner.get(&(trace, parent)) else {
                continue;
            };
            // A service calling itself is not a topology edge. It is real (an internal stage, an
            // async handoff) but drawing every service with a self-loop tells you nothing about
            // who depends on whom, which is the one question this view exists to answer.
            if parent_service == child_service {
                continue;
            }
            let acc = edges.entry((parent_service, child_service)).or_default();
            acc.count += 1;
            if is_error(c.status, i) {
                acc.errors += 1;
            }
            if !c.dur.is_null(i) {
                acc.durations.push(c.dur.value(i));
            }
        }
    }

    let capped = scanned >= MAX_MAP_SPANS;
    if capped {
        eprintln!(
            "photon-query: service_map hit the {MAX_MAP_SPANS}-span scan cap; edges may be \
             missing or undercounted (reported as `capped` on the response)"
        );
    }

    Ok(assemble(nodes, edges, focus, scanned, capped))
}

/// Turn the two fold maps into the sorted, focus-narrowed result.
fn assemble(
    nodes: HashMap<&str, NodeAcc>,
    edges: HashMap<(&str, &str), EdgeAcc>,
    focus: Option<&str>,
    scanned: usize,
    capped: bool,
) -> ServiceMap {
    let mut out_edges: Vec<MapEdge> = edges
        .into_iter()
        .filter(|((from, to), _)| match focus {
            // Both directions: "what does x call" and "who calls x" are the same question asked
            // from two sides, and a map that answered only one would be a list.
            Some(f) => *from == f || *to == f,
            None => true,
        })
        .map(|((from, to), mut acc)| {
            acc.durations.sort_unstable();
            MapEdge {
                from: from.to_string(),
                to: to.to_string(),
                count: acc.count,
                error_count: acc.errors,
                p50: pct(&acc.durations, 0.5),
                p95: pct(&acc.durations, 0.95),
                p99: pct(&acc.durations, 0.99),
            }
        })
        .collect();
    out_edges.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });

    // Focused: only the services on the surviving edges, plus the focus itself — which must appear
    // even when it has no edges at all, or a service that talks to nobody would render as a blank
    // page rather than as an isolated node.
    let mut out_nodes: Vec<MapNode> = nodes
        .into_iter()
        .filter(|(service, _)| match focus {
            Some(f) => {
                *service == f
                    || out_edges
                        .iter()
                        .any(|e| e.from == *service || e.to == *service)
            }
            None => true,
        })
        .map(|(service, acc)| MapNode {
            service: service.to_string(),
            count: acc.count,
            error_count: acc.errors,
            roots: acc.roots,
        })
        .collect();
    out_nodes.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.service.cmp(&b.service))
    });

    ServiceMap {
        nodes: out_nodes,
        edges: out_edges,
        scanned,
        capped,
    }
}

/// One batch's borrowed columns.
struct Cols<'a> {
    trace: &'a StringArray,
    span: &'a StringArray,
    parent: &'a StringArray,
    service: &'a StringArray,
    dur: &'a Int64Array,
    status: &'a Int32Array,
    rows: usize,
}

fn is_error(status: &Int32Array, i: usize) -> bool {
    !status.is_null(i) && status.value(i) == 2
}

/// The `i`-th value if non-null and non-empty. An empty `parent_span_id` is how a root span is
/// written by some producers, and treating `""` as a real parent would invent an edge to nowhere.
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

/// Exact nearest-rank percentile over an already-sorted slice (nanoseconds). Empty → 0. Same rule
/// as `service_dependencies::pct`, so an edge and a dependency row mean the same thing by "p95".
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
            PhotonError::Query(format!("service_map column `{name}` missing or not Utf8"))
        })
}

fn i64_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int64Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| {
            PhotonError::Query(format!("service_map column `{name}` missing or not Int64"))
        })
}

fn i32_col<'a>(b: &'a RecordBatch, name: &str) -> Result<&'a Int32Array, PhotonError> {
    b.column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .ok_or_else(|| {
            PhotonError::Query(format!("service_map column `{name}` missing or not Int32"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::{lit, SessionContext};

    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    fn schema() -> SpanSchema {
        SpanSchema::new(&["service.name".into()])
    }

    #[allow(clippy::too_many_arguments)]
    fn span(
        service: &str,
        trace: &str,
        span_id: &str,
        parent: Option<&str>,
        dur: Option<i64>,
        status: Option<i32>,
    ) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        SpanRecord {
            trace_id: trace.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent.map(|p| p.to_string()),
            name: Some("op".into()),
            start_time_nanos: 1_000,
            duration_nanos: dur,
            status_code: status,
            attributes,
            ..Default::default()
        }
    }

    async fn df_of(records: &[SpanRecord]) -> DataFrame {
        let s = schema();
        let mut b = SpanBatchBuilder::new(&s);
        for r in records {
            b.append(r);
        }
        let ctx = SessionContext::new();
        ctx.register_table(
            "spans",
            Arc::new(MemTable::try_new(s.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap()),
        )
        .unwrap();
        ctx.table("spans").await.unwrap()
    }

    async fn map(records: &[SpanRecord], focus: Option<&str>) -> ServiceMap {
        map_over(df_of(records).await, lit(true), focus)
            .await
            .unwrap()
    }

    fn edge<'a>(m: &'a ServiceMap, from: &str, to: &str) -> Option<&'a MapEdge> {
        m.edges.iter().find(|e| e.from == from && e.to == to)
    }

    #[tokio::test]
    async fn edges_are_derived_from_cross_service_parentage() {
        let m = map(
            &[
                span("gateway", "t1", "s1", None, Some(500), Some(1)),
                span("api", "t1", "s2", Some("s1"), Some(300), Some(1)),
                span("db-proxy", "t1", "s3", Some("s2"), Some(100), Some(1)),
            ],
            None,
        )
        .await;
        assert_eq!(m.edges.len(), 2);
        assert!(edge(&m, "gateway", "api").is_some());
        assert!(edge(&m, "api", "db-proxy").is_some());
    }

    #[tokio::test]
    async fn same_service_parent_child_is_not_an_edge() {
        let m = map(
            &[
                span("api", "t1", "s1", None, Some(500), Some(1)),
                span("api", "t1", "s2", Some("s1"), Some(300), Some(1)),
            ],
            None,
        )
        .await;
        assert!(m.edges.is_empty());
        // ...but the service is still a node, with both its spans counted.
        assert_eq!(m.nodes.len(), 1);
        assert_eq!(m.nodes[0].count, 2);
    }

    #[tokio::test]
    async fn service_param_returns_both_directions() {
        let m = map(
            &[
                span("gateway", "t1", "s1", None, Some(500), Some(1)),
                span("api", "t1", "s2", Some("s1"), Some(300), Some(1)),
                span("billing", "t1", "s3", Some("s2"), Some(100), Some(1)),
                // An edge nowhere near `api` — must not survive the focus.
                span("cron", "t2", "s4", None, Some(10), Some(1)),
                span("mailer", "t2", "s5", Some("s4"), Some(10), Some(1)),
            ],
            Some("api"),
        )
        .await;
        assert_eq!(m.edges.len(), 2);
        assert!(edge(&m, "gateway", "api").is_some()); // who calls api
        assert!(edge(&m, "api", "billing").is_some()); // what api calls
        assert!(edge(&m, "cron", "mailer").is_none());
        // Only the services on the surviving edges are nodes.
        let names: Vec<&str> = m.nodes.iter().map(|n| n.service.as_str()).collect();
        assert!(!names.contains(&"cron"));
    }

    #[tokio::test]
    async fn a_focused_service_with_no_edges_is_still_a_node() {
        // Otherwise "this service talks to nobody" renders as a blank page.
        let m = map(
            &[span("lonely", "t1", "s1", None, Some(5), Some(1))],
            Some("lonely"),
        )
        .await;
        assert!(m.edges.is_empty());
        assert_eq!(m.nodes.len(), 1);
        assert_eq!(m.nodes[0].service, "lonely");
    }

    #[tokio::test]
    async fn an_edges_latency_is_the_callees_own_duration() {
        let m = map(
            &[
                span("gateway", "t1", "s1", None, Some(9_000), Some(1)),
                span("api", "t1", "s2", Some("s1"), Some(100), Some(1)),
                span("api", "t2", "s4", Some("s3"), Some(300), Some(1)),
                span("gateway", "t2", "s3", None, Some(9_000), Some(1)),
            ],
            None,
        )
        .await;
        let e = edge(&m, "gateway", "api").unwrap();
        assert_eq!(e.count, 2);
        // The caller's 9,000ns spans are nowhere in these numbers.
        assert_eq!(e.p50, 100);
        assert_eq!(e.p99, 300);
    }

    #[tokio::test]
    async fn an_edge_counts_the_callees_errors() {
        let m = map(
            &[
                span("gateway", "t1", "s1", None, Some(500), Some(1)),
                span("api", "t1", "s2", Some("s1"), Some(300), Some(2)),
                span("gateway", "t2", "s3", None, Some(500), Some(1)),
                span("api", "t2", "s4", Some("s3"), Some(300), Some(1)),
            ],
            None,
        )
        .await;
        let e = edge(&m, "gateway", "api").unwrap();
        assert_eq!(e.count, 2);
        assert_eq!(e.error_count, 1);
    }

    #[tokio::test]
    async fn a_parent_outside_the_scan_produces_no_edge() {
        // We do not know whose span `missing` was, so inventing an edge would be a guess.
        let m = map(
            &[span("api", "t1", "s2", Some("missing"), Some(300), Some(1))],
            None,
        )
        .await;
        assert!(m.edges.is_empty());
        assert_eq!(m.nodes.len(), 1);
    }

    #[tokio::test]
    async fn an_empty_parent_string_is_a_root_not_an_edge_to_nowhere() {
        let m = map(
            &[span("api", "t1", "s1", Some(""), Some(300), Some(1))],
            None,
        )
        .await;
        assert!(m.edges.is_empty());
        assert_eq!(m.nodes[0].roots, 1);
    }

    #[tokio::test]
    async fn the_same_span_id_in_two_traces_does_not_cross_wires() {
        // `span_id` is unique only WITHIN a trace per OTLP — the join key must be the pair.
        let m = map(
            &[
                span("gateway", "t1", "s1", None, Some(500), Some(1)),
                span("api", "t1", "s2", Some("s1"), Some(300), Some(1)),
                span("cron", "t2", "s1", None, Some(500), Some(1)),
                span("mailer", "t2", "s2", Some("s1"), Some(300), Some(1)),
            ],
            None,
        )
        .await;
        assert_eq!(m.edges.len(), 2);
        assert!(edge(&m, "gateway", "api").is_some());
        assert!(edge(&m, "cron", "mailer").is_some());
        assert!(edge(&m, "gateway", "mailer").is_none());
    }

    #[tokio::test]
    async fn nodes_carry_their_own_totals_not_just_edge_totals() {
        let m = map(
            &[
                span("gateway", "t1", "s1", None, Some(500), Some(1)),
                span("api", "t1", "s2", Some("s1"), Some(300), Some(2)),
                // An internal span with no cross-service parentage still counts toward the node.
                span("api", "t1", "s3", Some("s2"), Some(50), Some(1)),
            ],
            None,
        )
        .await;
        let api = m.nodes.iter().find(|n| n.service == "api").unwrap();
        assert_eq!(api.count, 2);
        assert_eq!(api.error_count, 1);
        assert_eq!(api.roots, 0);
        let gw = m.nodes.iter().find(|n| n.service == "gateway").unwrap();
        assert_eq!(gw.roots, 1);
    }

    #[tokio::test]
    async fn an_empty_window_returns_no_edges() {
        let m = map(&[], None).await;
        assert!(m.edges.is_empty());
        assert!(m.nodes.is_empty());
        assert_eq!(m.scanned, 0);
        assert!(!m.capped);
    }

    #[tokio::test]
    async fn a_scan_under_the_cap_is_not_reported_as_capped() {
        let m = map(&[span("api", "t1", "s1", None, Some(1), Some(1))], None).await;
        assert_eq!(m.scanned, 1);
        assert!(!m.capped);
    }

    #[test]
    fn the_cap_is_reported_rather_than_silently_truncating() {
        // The cap itself is not exercised with 300k rows in a unit test; what IS pinned is that
        // `capped` is derived from the scan count reaching it, so a truncated map can never be
        // reported as a complete one.
        let empty: HashMap<&str, NodeAcc> = HashMap::new();
        let m = assemble(empty, HashMap::new(), None, MAX_MAP_SPANS, true);
        assert!(m.capped);
        assert_eq!(m.scanned, MAX_MAP_SPANS);
    }
}
