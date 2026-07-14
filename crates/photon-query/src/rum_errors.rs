//! RUM error issues: group JS-error logs (severity ERROR) by their `rum.error.fingerprint`
//! attribute into "issues" — one row per fingerprint with an occurrence count, an exact
//! distinct-session count, and a sample `exception.type` / `exception.message`.
//!
//! Errors are stored as ordinary `LogRecord`s (`severity_number = 17`, OTEL ERROR range 17–24)
//! whose fingerprint / session / exception attributes live in the long-tail attributes map
//! (none are promoted), so every grouping key is read with `get_field(attributes, key)` — the
//! same map access the log grammar/facet compiles to. Mirrors the group-by-value + count +
//! order-by-count-desc + limit shape of `facet.rs`, with a pure `issues_over(df, limit)` seam
//! that unit tests drive over an in-memory `MemTable`.

use arrow::array::{Array, Int64Array, StringArray};
use arrow::datatypes::DataType;
use datafusion::dataframe::DataFrame;
use datafusion::functions::core::expr_fn::get_field;
use datafusion::functions_aggregate::expr_fn::{count, count_distinct, max, min};
use datafusion::prelude::{cast, col, lit};

use photon_core::query::ResolvedQuery;
use photon_core::rum::{
    ATTR_BROWSER, ATTR_CONNECTION, ATTR_DEVICE, ATTR_ERR_KIND, ATTR_EXC_MSG, ATTR_EXC_STACK,
    ATTR_EXC_TYPE, ATTR_FINGERPRINT, ATTR_ROUTE, ATTR_SESSION,
};
use photon_core::schema;
use photon_core::PhotonError;

use crate::facet::FacetValue;
use crate::{base_predicate, col_ref, QueryEngine, QueryRequest};

/// Occurrence-series resolution for the detail chart.
const SERIES_BUCKETS: usize = 48;

/// One error "issue": all ERROR logs sharing a `rum.error.fingerprint`, aggregated. `count` is
/// the total occurrences and `sessions` the exact number of distinct `session.id`s affected;
/// `exception_type` / `message` are a stable sample (the lexicographic min over the group).
pub struct ErrorIssue {
    /// The `rum.error.fingerprint` value that identifies this issue.
    pub fingerprint: String,
    /// A representative `exception.type` for the issue (empty if none recorded).
    pub exception_type: String,
    /// A representative `exception.message` for the issue (empty if none recorded).
    pub message: String,
    /// Total occurrences (matching ERROR log rows) in the window.
    pub count: i64,
    /// Exact number of distinct `session.id`s that hit this issue.
    pub sessions: i64,
    /// A representative `trace_id` (lexicographic `min` over the group; may be `None` if no error
    /// in the group carried a trace) — lights the list-row "Related ▾" trace jump.
    pub trace_id: Option<String>,
}

/// Full read-only detail for one error issue (one `rum.error.fingerprint`).
pub struct ErrorDetail {
    pub fingerprint: String,
    pub exception_type: String,
    pub message: String,
    pub error_kind: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub occurrences: i64,
    pub sessions: i64,
    /// Occurrences over time (equal-width buckets across the window).
    pub series: Vec<CountBucket>,
    /// Top values + counts for each tag field (`browser.name`/`device.type`/`browser.route`/`network.connection`).
    pub tags: Vec<TagBreakdown>,
    /// A representative raw `exception.stacktrace` (from the most recent sample event that has one).
    pub sample_stack: Option<String>,
    /// The N most-recent individual occurrences, each with its own `trace_id`.
    pub events: Vec<ErrorEvent>,
}

/// One time bucket: bucket-start (epoch nanos) + occurrence count.
pub struct CountBucket {
    pub t: i64,
    pub count: u64,
}

/// Top values for one tag field over the issue's rows.
pub struct TagBreakdown {
    pub field: String,
    pub values: Vec<FacetValue>,
}

/// One individual error occurrence — drives the sample-events table's per-event jumps.
pub struct ErrorEvent {
    pub timestamp: i64,
    pub route: String,
    pub browser: String,
    pub device: String,
    pub session: String,
    pub trace_id: Option<String>,
}

impl QueryEngine {
    /// Top `limit` error issues for `service` over `[start_ns, end_ns]`, grouped by
    /// `rum.error.fingerprint` and ordered by occurrence count desc. Only ERROR-severity rows
    /// (17–24) are considered. Returns an empty vec when no file survives pruning.
    ///
    /// `route`, when `Some(r)`, scopes the issues to rows whose `browser.route` attribute equals
    /// `r` — used by the page-detail view. `None` returns issues across the whole app.
    ///
    /// `query`, when `Some`, is an already-resolved log-grammar filter (e.g. from the UI's `q`
    /// search param) ANDed on top via `base_predicate` — the same fold the Logs search and
    /// `/api/facet` endpoints rely on.
    pub async fn rum_errors(
        &self,
        service: &str,
        start_ns: i64,
        end_ns: i64,
        limit: usize,
        route: Option<&str>,
        query: Option<ResolvedQuery>,
    ) -> Result<Vec<ErrorIssue>, PhotonError> {
        let req = QueryRequest {
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            services: vec![service.to_string()],
            severities: vec![(17, 24)], // OTEL ERROR range
            text: None,
            query,
            limit,
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(Vec::new());
        };
        let mut pred = base_predicate(&req);
        if let Some(r) = route {
            // Scope to one page: `attributes['browser.route'] = r` (map attribute, collapsed
            // through `IS TRUE` exactly like the map-attribute predicates in `predicate.rs`).
            pred = pred.and(
                get_field(col_ref(schema::ATTRIBUTES), ATTR_ROUTE)
                    .eq(lit(r))
                    .is_true(),
            );
        }
        let df = df
            .filter(pred)
            .map_err(|e| PhotonError::Query(format!("rum_errors filter: {e}")))?;
        issues_over(df, limit).await
    }

    /// Full detail for one error issue: header stats, occurrence series, tag breakdowns, a sample
    /// stack, and the N most-recent sample events (each with its own `trace_id`). Scoped by
    /// `service`, ERROR severity (17–24), the `rum.error.fingerprint` attribute, and the window.
    /// Returns an all-empty detail when nothing survives pruning / the fingerprint has no rows.
    pub async fn rum_error_detail(
        &self,
        service: &str,
        fingerprint: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<ErrorDetail, PhotonError> {
        let req = QueryRequest {
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            services: vec![service.to_string()],
            severities: vec![(17, 24)],
            text: None,
            query: None,
            limit: 0,
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(empty_detail(fingerprint));
        };
        let pred = base_predicate(&req).and(
            get_field(col_ref(schema::ATTRIBUTES), ATTR_FINGERPRINT)
                .eq(lit(fingerprint))
                .is_true(),
        );
        let df = df
            .filter(pred)
            .map_err(|e| PhotonError::Query(format!("rum_error_detail filter: {e}")))?;
        detail_over(df, fingerprint, start_ns, end_ns, 20).await
    }
}

/// GROUP BY the fingerprint attribute, COUNT occurrences + exact COUNT(DISTINCT session.id), plus
/// a sample `exception.type` / `exception.message` (lexicographic `min`), drop the NULL-fingerprint
/// group, order by count desc, and take `limit`. `df` must already carry the row predicate (the
/// caller applies `base_predicate`), mirroring `facet::facet_over`'s pure-seam shape.
pub(crate) async fn issues_over(
    df: DataFrame,
    limit: usize,
) -> Result<Vec<ErrorIssue>, PhotonError> {
    let fp = get_field(col_ref(schema::ATTRIBUTES), ATTR_FINGERPRINT);
    let session = get_field(col_ref(schema::ATTRIBUTES), ATTR_SESSION);
    let etype = get_field(col_ref(schema::ATTRIBUTES), ATTR_EXC_TYPE);
    let emsg = get_field(col_ref(schema::ATTRIBUTES), ATTR_EXC_MSG);
    let trace = col_ref(schema::TRACE_ID);

    let batches = df
        .aggregate(
            vec![fp.alias("fp")],
            vec![
                count(lit(1_i64)).alias("count"),
                count_distinct(session).alias("sessions"),
                min(etype).alias("etype"),
                min(emsg).alias("emsg"),
                min(trace).alias("trace"),
            ],
        )
        .map_err(|e| PhotonError::Query(format!("rum_errors aggregate: {e}")))?
        .filter(col("fp").is_not_null())
        .map_err(|e| PhotonError::Query(format!("rum_errors not-null: {e}")))?
        .sort(vec![
            col("count").sort(false, false), // count desc
            col("fp").sort(true, false),     // fingerprint asc — stable tiebreak
        ])
        .map_err(|e| PhotonError::Query(format!("rum_errors sort: {e}")))?
        .limit(0, Some(limit))
        .map_err(|e| PhotonError::Query(format!("rum_errors limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("rum_errors collect: {e}")))?;

    let mut out = Vec::new();
    for b in &batches {
        let str_col = |c: usize| -> Result<&StringArray, PhotonError> {
            b.column(c)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PhotonError::Query(format!("rum_errors: column {c} not Utf8")))
        };
        let int_col = |c: usize| -> Result<&Int64Array, PhotonError> {
            b.column(c)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| PhotonError::Query(format!("rum_errors: column {c} not Int64")))
        };
        let fp = str_col(0)?;
        let count = int_col(1)?;
        let sessions = int_col(2)?;
        let etype = str_col(3)?;
        let emsg = str_col(4)?;
        let trace = str_col(5)?;
        for i in 0..b.num_rows() {
            out.push(ErrorIssue {
                fingerprint: fp.value(i).to_string(),
                count: if count.is_null(i) { 0 } else { count.value(i) },
                sessions: if sessions.is_null(i) {
                    0
                } else {
                    sessions.value(i)
                },
                exception_type: if etype.is_null(i) {
                    String::new()
                } else {
                    etype.value(i).to_string()
                },
                message: if emsg.is_null(i) {
                    String::new()
                } else {
                    emsg.value(i).to_string()
                },
                trace_id: if trace.is_null(i) {
                    None
                } else {
                    Some(trace.value(i).to_string())
                },
            });
        }
    }
    Ok(out)
}

/// An all-empty detail (no rows in range) — the API returns this as a 200 with empty sections.
fn empty_detail(fingerprint: &str) -> ErrorDetail {
    ErrorDetail {
        fingerprint: fingerprint.to_string(),
        exception_type: String::new(),
        message: String::new(),
        error_kind: String::new(),
        first_seen: 0,
        last_seen: 0,
        occurrences: 0,
        sessions: 0,
        series: Vec::new(),
        tags: Vec::new(),
        sample_stack: None,
        events: Vec::new(),
    }
}

/// Compose the detail sections over a df already filtered to one fingerprint's ERROR rows.
pub(crate) async fn detail_over(
    df: DataFrame,
    fingerprint: &str,
    start_ns: i64,
    end_ns: i64,
    sample_n: usize,
) -> Result<ErrorDetail, PhotonError> {
    let mut detail = empty_detail(fingerprint);

    // --- header: one aggregate over the fingerprint-filtered rows ---
    let session = get_field(col_ref(schema::ATTRIBUTES), ATTR_SESSION);
    let etype = get_field(col_ref(schema::ATTRIBUTES), ATTR_EXC_TYPE);
    let emsg = get_field(col_ref(schema::ATTRIBUTES), ATTR_EXC_MSG);
    let kind = get_field(col_ref(schema::ATTRIBUTES), ATTR_ERR_KIND);
    let ts = cast(col_ref(schema::TIMESTAMP), DataType::Int64);

    let batches = df
        .clone()
        .aggregate(
            vec![],
            vec![
                count(lit(1_i64)).alias("occurrences"),
                count_distinct(session).alias("sessions"),
                min(ts.clone()).alias("first_seen"),
                max(ts).alias("last_seen"),
                min(etype).alias("etype"),
                min(emsg).alias("emsg"),
                min(kind).alias("kind"),
            ],
        )
        .map_err(|e| PhotonError::Query(format!("rum_error_detail header aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("rum_error_detail header collect: {e}")))?;

    if let Some(b) = batches.first() {
        if b.num_rows() > 0 {
            let i = 0;
            let int = |c: usize| b.column(c).as_any().downcast_ref::<Int64Array>();
            let s = |c: usize| b.column(c).as_any().downcast_ref::<StringArray>();
            let iv = |c: usize| {
                int(c)
                    .map(|a| if a.is_null(i) { 0 } else { a.value(i) })
                    .unwrap_or(0)
            };
            let sv = |c: usize| {
                s(c).filter(|a| !a.is_null(i))
                    .map(|a| a.value(i).to_string())
                    .unwrap_or_default()
            };
            detail.occurrences = iv(0);
            detail.sessions = iv(1);
            detail.first_seen = iv(2);
            detail.last_seen = iv(3);
            detail.exception_type = sv(4);
            detail.message = sv(5);
            detail.error_kind = sv(6);
        }
    }

    // --- occurrences over time: reuse the histogram bucketing, keeping only the per-bucket total ---
    detail.series =
        crate::histogram::histogram_over(df.clone(), lit(true), start_ns, end_ns, SERIES_BUCKETS)
            .await?
            .into_iter()
            .map(|h| CountBucket {
                t: h.t,
                count: h.total,
            })
            .collect();

    // --- tag breakdowns: top values per field, reusing facet_over's group-by-count shape ---
    const TAG_FIELDS: [&str; 4] = [ATTR_BROWSER, ATTR_DEVICE, ATTR_ROUTE, ATTR_CONNECTION];
    let mut tags = Vec::with_capacity(TAG_FIELDS.len());
    for field in TAG_FIELDS {
        let value_expr = get_field(col_ref(schema::ATTRIBUTES), field);
        let fr = crate::facet::facet_over(df.clone(), lit(true), value_expr, 8).await?;
        tags.push(TagBreakdown {
            field: field.to_string(),
            values: fr.values,
        });
    }
    detail.tags = tags;

    // --- sample events: the N most-recent occurrences + a representative stack ---
    let (events, sample_stack) = detail_events(df.clone(), sample_n).await?;
    detail.events = events;
    detail.sample_stack = sample_stack;

    Ok(detail)
}

/// The `n` most-recent rows of `df`, projected into `ErrorEvent`s (each with its own native
/// `trace_id`), plus a representative raw stack (the first non-empty `exception.stacktrace`).
async fn detail_events(
    df: DataFrame,
    n: usize,
) -> Result<(Vec<ErrorEvent>, Option<String>), PhotonError> {
    let route = get_field(col_ref(schema::ATTRIBUTES), ATTR_ROUTE).alias("route");
    let browser = get_field(col_ref(schema::ATTRIBUTES), ATTR_BROWSER).alias("browser");
    let device = get_field(col_ref(schema::ATTRIBUTES), ATTR_DEVICE).alias("device");
    let session = get_field(col_ref(schema::ATTRIBUTES), ATTR_SESSION).alias("session");
    let stack = get_field(col_ref(schema::ATTRIBUTES), ATTR_EXC_STACK).alias("stack");

    let batches = df
        .select(vec![
            cast(col_ref(schema::TIMESTAMP), DataType::Int64).alias("ts"),
            col_ref(schema::TRACE_ID).alias("trace"),
            route,
            browser,
            device,
            session,
            stack,
        ])
        .map_err(|e| PhotonError::Query(format!("rum_error_detail events select: {e}")))?
        .sort(vec![col("ts").sort(false, false)]) // most recent first
        .map_err(|e| PhotonError::Query(format!("rum_error_detail events sort: {e}")))?
        .limit(0, Some(n))
        .map_err(|e| PhotonError::Query(format!("rum_error_detail events limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("rum_error_detail events collect: {e}")))?;

    let mut events = Vec::new();
    let mut sample_stack: Option<String> = None;
    for b in &batches {
        let int_col = |c: usize| -> Result<&Int64Array, PhotonError> {
            b.column(c)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| {
                    PhotonError::Query(format!("rum_error_detail events col {c} not Int64"))
                })
        };
        let str_col = |c: usize| -> Result<&StringArray, PhotonError> {
            b.column(c)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    PhotonError::Query(format!("rum_error_detail events col {c} not Utf8"))
                })
        };
        let ts = int_col(0)?;
        let trace = str_col(1)?;
        let route = str_col(2)?;
        let browser = str_col(3)?;
        let device = str_col(4)?;
        let session = str_col(5)?;
        let stack = str_col(6)?;
        for i in 0..b.num_rows() {
            let s = |a: &StringArray| {
                if a.is_null(i) {
                    String::new()
                } else {
                    a.value(i).to_string()
                }
            };
            if sample_stack.is_none() && !stack.is_null(i) && !stack.value(i).is_empty() {
                sample_stack = Some(stack.value(i).to_string());
            }
            events.push(ErrorEvent {
                timestamp: if ts.is_null(i) { 0 } else { ts.value(i) },
                route: s(route),
                browser: s(browser),
                device: s(device),
                session: s(session),
                trace_id: if trace.is_null(i) {
                    None
                } else {
                    Some(trace.value(i).to_string())
                },
            });
        }
    }
    Ok((events, sample_stack))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::rum::ERROR_SEVERITY_NUMBER;
    use photon_core::schema::LogSchema;

    /// One ERROR log row for issue `fp`, seen in session `session`. `service.name` is the only
    /// promoted attribute; the fingerprint / session / exception keys are long-tail map entries.
    fn err(fp: &str, session: &str) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), "web".to_string());
        attributes.insert(ATTR_FINGERPRINT.into(), fp.to_string());
        attributes.insert(ATTR_SESSION.into(), session.to_string());
        attributes.insert(ATTR_EXC_TYPE.into(), "TypeError".to_string());
        attributes.insert(ATTR_EXC_MSG.into(), format!("boom {fp}"));
        attributes.insert(ATTR_ROUTE.into(), "/checkout".to_string());
        attributes.insert(ATTR_BROWSER.into(), "Chrome".to_string());
        attributes.insert(ATTR_DEVICE.into(), "mobile".to_string());
        attributes.insert(ATTR_EXC_STACK.into(), "at f (a.js:10)".to_string());
        LogRecord {
            timestamp_nanos: 1_000,
            severity_number: Some(ERROR_SEVERITY_NUMBER),
            body: Some("boom".into()),
            trace_id: Some(format!("{:032x}", fp.bytes().next().unwrap_or(0))),
            attributes,
            ..Default::default()
        }
    }

    /// Build an in-memory `logs` MemTable from hand-built ERROR records and run the pure
    /// `issues_over` seam over it (no pruning, no Parquet) — the facet.rs test pattern.
    async fn run_rum_errors(records: Vec<LogRecord>) -> Vec<ErrorIssue> {
        let schema = LogSchema::new(&["service.name".into()]);
        let mut b = RecordBatchBuilder::new(&schema);
        for r in &records {
            b.append(r);
        }
        let ctx = SessionContext::new();
        ctx.register_table(
            "logs",
            Arc::new(
                MemTable::try_new(schema.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap(),
            ),
        )
        .unwrap();
        let df = ctx.table("logs").await.unwrap();
        issues_over(df, 50).await.unwrap()
    }

    #[tokio::test]
    async fn errors_group_by_fingerprint_with_distinct_sessions() {
        let issues = run_rum_errors(vec![
            err("A", "s1"),
            err("A", "s1"),
            err("A", "s2"),
            err("B", "s3"),
        ])
        .await;
        assert_eq!(issues.len(), 2);
        let a = issues.iter().find(|i| i.fingerprint == "A").unwrap();
        assert_eq!(a.count, 3);
        assert_eq!(a.sessions, 2);
        // A sample exception type/message is carried through for display.
        assert_eq!(a.exception_type, "TypeError");
        assert!(a.message.starts_with("boom"));
        // A representative trace id is carried for the list-row Related-menu hook.
        assert_eq!(
            a.trace_id.as_deref(),
            Some(format!("{:032x}", b'A').as_str())
        );
        let b = issues.iter().find(|i| i.fingerprint == "B").unwrap();
        assert_eq!(b.count, 1);
        assert_eq!(b.sessions, 1);
    }

    /// Build a `logs` MemTable of ERROR rows for one fingerprint and drive `detail_over` directly.
    async fn run_detail(records: Vec<LogRecord>, start: i64, end: i64) -> ErrorDetail {
        let schema = LogSchema::new(&["service.name".into()]);
        let mut b = RecordBatchBuilder::new(&schema);
        for r in &records {
            b.append(r);
        }
        let ctx = SessionContext::new();
        ctx.register_table(
            "logs",
            Arc::new(
                MemTable::try_new(schema.arrow.clone(), vec![vec![b.finish().unwrap()]]).unwrap(),
            ),
        )
        .unwrap();
        let df = ctx.table("logs").await.unwrap();
        detail_over(df, "A", start, end, 20).await.unwrap()
    }

    fn err_at(fp: &str, session: &str, ts: i64) -> LogRecord {
        let mut r = err(fp, session);
        r.timestamp_nanos = ts;
        r
    }

    #[tokio::test]
    async fn detail_header_and_series() {
        let d = run_detail(
            vec![
                err_at("A", "s1", 1_000),
                err_at("A", "s1", 2_000),
                err_at("A", "s2", 3_000),
            ],
            0,
            4_000,
        )
        .await;
        assert_eq!(d.fingerprint, "A");
        assert_eq!(d.occurrences, 3);
        assert_eq!(d.sessions, 2);
        assert_eq!(d.first_seen, 1_000);
        assert_eq!(d.last_seen, 3_000);
        assert_eq!(d.exception_type, "TypeError");
        assert_eq!(d.series.iter().map(|c| c.count).sum::<u64>(), 3);
    }

    #[tokio::test]
    async fn detail_tags_events_and_stack() {
        let d = run_detail(
            vec![err_at("A", "s1", 1_000), err_at("A", "s2", 3_000)],
            0,
            4_000,
        )
        .await;
        // Tag breakdown for browser.name has Chrome=2.
        let browser = d.tags.iter().find(|t| t.field == "browser.name").unwrap();
        assert_eq!(
            browser
                .values
                .iter()
                .find(|v| v.value == "Chrome")
                .unwrap()
                .count,
            2
        );
        // Sample events are most-recent-first, carry a trace_id, and a representative stack is picked.
        assert_eq!(d.events.len(), 2);
        assert_eq!(d.events[0].timestamp, 3_000);
        assert_eq!(d.events[0].browser, "Chrome");
        assert!(d.events[0].trace_id.is_some());
        assert_eq!(d.sample_stack.as_deref(), Some("at f (a.js:10)"));
    }
}
