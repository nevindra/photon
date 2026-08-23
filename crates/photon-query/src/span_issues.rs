//! **Backend Issues**: errored spans grouped by `(service.name, error.fingerprint)`.
//!
//! Task 14 (`photon_core::span_error`) writes `error.type` / `error.message` /
//! `error.fingerprint` onto every errored span at ingest. This is the read side: it groups those
//! spans into Issues, the same way `crate::rum_errors` groups errored RUM logs into Frontend
//! Issues. The two surfaces are deliberately shaped alike — an engineer who has read one list
//! already knows how to read the other.
//!
//! **Identity is the pair `(service, fingerprint)`, never the fingerprint alone.** The fingerprint
//! hashes the exception type, its message and its top frame, and nothing about where it ran; that
//! is on purpose (see `span_error`'s module docs), because it lets you see that one bug is present
//! in three services. But three services is three Issues: three owners, three deploys, three
//! fixes. So every grouping here carries `service.name` in the key, including on the per-service
//! routes where it is degenerate — one grouping rule, not one per caller.
//!
//! **The stack trace is not an attribute.** Ingest stores `exception.stacktrace` inside the span's
//! `events` JSON and does not copy it out; a stack trace is kilobytes and would be paid for on
//! every read of every column. The detail view therefore parses `events` for its ≤20 sample rows
//! only, which is the one place a human actually wants to read a trace.
use arrow::array::{Array, Int32Array, Int64Array, StringArray};
use arrow::datatypes::DataType;
use datafusion::dataframe::DataFrame;
use datafusion::functions::core::expr_fn::get_field;
use datafusion::functions_aggregate::expr_fn::{count, count_distinct, max, min};
use datafusion::prelude::{cast, col, lit, Expr};

use photon_core::query::SpanFieldResolver;
use photon_core::span_error::{
    ERROR_FINGERPRINT, ERROR_MESSAGE, ERROR_TYPE, EXCEPTION_EVENT, EXCEPTION_STACKTRACE,
};
use photon_core::span_schema;
use photon_core::PhotonError;

use crate::facet::FacetValue;
use crate::span_engine::span_base_predicate;
use crate::span_predicate::span_field_col;
use crate::{col_ref, SpanQueryEngine, SpanQueryRequest};

/// Occurrence-series resolution for the detail chart. Same number as `rum_errors`, so the two
/// issue-detail charts have the same granularity for the same window.
const SERIES_BUCKETS: usize = 48;

/// Top-N values kept per tag field on the detail.
const TAG_LIMIT: usize = 8;

/// The dimensions the Issues list can be filtered/browsed by. Six, mirroring `rum_errors`'
/// `ERROR_FACET_FIELDS`, chosen for the backend's own questions: what threw, where it threw, which
/// environment and host, and what the caller saw.
pub const ISSUE_FACET_FIELDS: [&str; 6] = [
    ERROR_TYPE,
    "name",
    "kind_text",
    "deployment.environment",
    "host.name",
    "http.status_code",
];

/// The dimensions broken down on one Issue's detail. `error.type` is absent on purpose: it is
/// constant within an Issue, so a breakdown of it is one bar reading 100%.
pub const ISSUE_TAG_FIELDS: [&str; 4] = [
    "name",
    "http.status_code",
    "deployment.environment",
    "host.name",
];

/// One Backend Issue: every errored span sharing `(service, fingerprint)` in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanIssue {
    /// The owning service. Half of the Issue's identity — see the module docs.
    pub service: String,
    /// `error.fingerprint`, written at ingest.
    pub fingerprint: String,
    /// A representative `error.type` (lexicographic `min` over the group).
    pub error_type: String,
    /// A representative `error.message`.
    pub message: String,
    /// Total errored spans in the window.
    pub count: i64,
    /// Distinct traces affected — the backend's answer to RUM's distinct-sessions. One trace is
    /// one request that went wrong, whereas `count` also counts a retry loop inside it.
    pub traces: i64,
    /// Distinct span names this Issue appears under. `> 1` means one fault reached through
    /// several operations, which is worth seeing on the row.
    pub operations: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    /// A representative `trace_id`, lighting the row's jump into a real failing trace.
    pub trace_id: Option<String>,
}

/// One time bucket of an Issue's occurrences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IssueBucket {
    pub t: i64,
    pub count: u64,
}

/// Top values for one tag field over the Issue's spans.
pub struct IssueTag {
    pub field: String,
    pub values: Vec<FacetValue>,
}

/// One individual occurrence — a single errored span, with everything needed to jump to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEvent {
    pub timestamp: i64,
    pub trace_id: String,
    pub span_id: String,
    pub operation: String,
    pub kind: String,
    pub duration_nanos: Option<i64>,
    pub status_message: String,
    pub host: String,
    pub environment: String,
}

/// Full read-only detail for one Backend Issue.
pub struct SpanIssueDetail {
    pub service: String,
    pub fingerprint: String,
    pub error_type: String,
    pub message: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub occurrences: i64,
    pub traces: i64,
    pub operations: i64,
    /// Occurrences over time (equal-width buckets across the window).
    pub series: Vec<IssueBucket>,
    /// Top values + counts for each of [`ISSUE_TAG_FIELDS`].
    pub tags: Vec<IssueTag>,
    /// A representative raw `exception.stacktrace`, dug out of the `events` JSON of the most
    /// recent sample that carries one. `None` when the producer sets `status = ERROR` without
    /// recording an exception — a real and common case, not an error.
    pub sample_stack: Option<String>,
    /// The N most-recent individual occurrences.
    pub events: Vec<IssueEvent>,
}

/// Mark the request as needing a non-null `error.fingerprint`, which every query in this module
/// does: the grouping key drops the null-fingerprint group, and only an errored span has one.
///
/// Saying so up front is what makes the Issues surface cheap. Ingest writes the key onto errored
/// spans only, so on a healthy fleet the vast majority of segments never contain it — and the
/// manifest already records, per segment, which long-tail keys it holds. Declaring the requirement
/// lets `keep_span_search_candidate` drop those segments before a single Parquet page is read,
/// instead of scanning them and finding nothing. Measured on 86.6M spans, 564 of 579 span segments
/// carry no `error.fingerprint` at all.
fn require_fingerprint(mut req: SpanQueryRequest) -> SpanQueryRequest {
    if !req
        .required_attributes
        .iter()
        .any(|k| k == ERROR_FINGERPRINT)
    {
        req.required_attributes.push(ERROR_FINGERPRINT.to_string());
    }
    req
}

impl SpanQueryEngine {
    /// Top `limit` Backend Issues matching `req`, grouped by `(service.name, error.fingerprint)`
    /// and ordered by occurrence count desc.
    ///
    /// `req` carries the window and the caller's scope — `service.name:api` for the per-service
    /// list, nothing extra for the fleet-wide one. There is no separate "errors only" filter
    /// because there does not need to be: a span without `error.fingerprint` is dropped by the
    /// not-null on the grouping key, and only an errored span has one.
    pub async fn span_issues(
        &self,
        req: SpanQueryRequest,
        limit: usize,
    ) -> Result<Vec<SpanIssue>, PhotonError> {
        // Defense-in-depth against a hand-crafted `limit`, mirroring every sibling engine:
        // `photon-api`'s `MAX_LIMIT` restated as a literal since `photon-query` cannot depend on
        // `photon-api`.
        let limit = limit.min(1000);
        let Some(df) = self
            .span_survivors_df(&require_fingerprint(req.clone()))
            .await?
        else {
            return Ok(Vec::new());
        };
        issues_over(df, span_base_predicate(&req), limit).await
    }

    /// Full detail for one Issue. `req` carries the window plus the caller's scope; `service` and
    /// `fingerprint` are applied here as an explicit equality pair so the identity rule lives in
    /// one place instead of in each caller's query string.
    pub async fn span_issue_detail(
        &self,
        req: SpanQueryRequest,
        service: &str,
        fingerprint: &str,
        sample_n: usize,
    ) -> Result<SpanIssueDetail, PhotonError> {
        let (start, end) = (req.start_ts_nanos, req.end_ts_nanos);
        let Some(df) = self
            .span_survivors_df(&require_fingerprint(req.clone()))
            .await?
        else {
            return Ok(empty_detail(service, fingerprint));
        };
        let pred = span_base_predicate(&req)
            .and(col_ref("service.name").eq(lit(service.to_string())))
            .and(
                get_field(col_ref(span_schema::ATTRIBUTES), ERROR_FINGERPRINT)
                    .eq(lit(fingerprint.to_string()))
                    .is_true(),
            );
        let df = df
            .filter(pred)
            .map_err(|e| PhotonError::Query(format!("span_issue_detail filter: {e}")))?;
        detail_over(
            df,
            self.promoted_attributes(),
            service,
            fingerprint,
            start,
            end,
            sample_n,
        )
        .await
    }
}

/// Resolve a field name to its value `Expr` through the same rules the spans grammar uses, so a
/// facet groups by exactly what a filter on that name would match. Falls back to a map lookup for
/// an unknown key, which is what `SpanFieldResolver` already does — a tag field that is promoted
/// on one deployment and long-tail on another still reads the same.
fn field_col(promoted: &[String], name: &str) -> Result<Expr, PhotonError> {
    let fr = SpanFieldResolver::new(promoted)
        .resolve_field_name(name)
        .map_err(|e| {
            PhotonError::Query(format!("cannot group Issues by `{name}`: {}", e.message))
        })?;
    Ok(span_field_col(&fr))
}

/// GROUP BY `(service.name, error.fingerprint)`, drop the null-fingerprint group (every healthy
/// span), order by count desc, take `limit`. Pure seam over a df — unit tests drive it with an
/// in-memory `MemTable`, no pruning and no Parquet, exactly like `rum_errors::issues_over`.
pub(crate) async fn issues_over(
    df: DataFrame,
    predicate: Expr,
    limit: usize,
) -> Result<Vec<SpanIssue>, PhotonError> {
    let attr = |k: &str| get_field(col_ref(span_schema::ATTRIBUTES), k);
    let ts = cast(col_ref(span_schema::START_TIME), DataType::Int64);

    let batches = df
        .filter(predicate)
        .map_err(|e| PhotonError::Query(format!("span_issues filter: {e}")))?
        .aggregate(
            vec![
                col_ref("service.name").alias("service"),
                attr(ERROR_FINGERPRINT).alias("fp"),
            ],
            vec![
                count(lit(1_i64)).alias("count"),
                count_distinct(col_ref(span_schema::TRACE_ID)).alias("traces"),
                count_distinct(col_ref(span_schema::NAME)).alias("operations"),
                min(ts.clone()).alias("first_seen"),
                max(ts).alias("last_seen"),
                min(attr(ERROR_TYPE)).alias("etype"),
                min(attr(ERROR_MESSAGE)).alias("emsg"),
                min(col_ref(span_schema::TRACE_ID)).alias("trace"),
            ],
        )
        .map_err(|e| PhotonError::Query(format!("span_issues aggregate: {e}")))?
        .filter(col("fp").is_not_null().and(col("service").is_not_null()))
        .map_err(|e| PhotonError::Query(format!("span_issues not-null: {e}")))?
        .sort(vec![
            col("count").sort(false, false),  // count desc
            col("service").sort(true, false), // stable tiebreak…
            col("fp").sort(true, false),      // …on the full identity
        ])
        .map_err(|e| PhotonError::Query(format!("span_issues sort: {e}")))?
        .limit(0, Some(limit))
        .map_err(|e| PhotonError::Query(format!("span_issues limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("span_issues collect: {e}")))?;

    let mut out = Vec::new();
    for b in &batches {
        let s = |c: usize| str_col(b, c, "span_issues");
        let i = |c: usize| int_col(b, c, "span_issues");
        let (service, fp) = (s(0)?, s(1)?);
        let (count, traces, operations) = (i(2)?, i(3)?, i(4)?);
        let (first, last) = (i(5)?, i(6)?);
        let (etype, emsg, trace) = (s(7)?, s(8)?, s(9)?);
        for r in 0..b.num_rows() {
            out.push(SpanIssue {
                service: text(service, r),
                fingerprint: text(fp, r),
                error_type: text(etype, r),
                message: text(emsg, r),
                count: num(count, r),
                traces: num(traces, r),
                operations: num(operations, r),
                first_seen: num(first, r),
                last_seen: num(last, r),
                trace_id: opt_text(trace, r),
            });
        }
    }
    Ok(out)
}

/// An all-empty detail (no spans in range) — the API returns this as a 200 with empty sections,
/// same as the RUM issue detail. A 404 would claim the Issue does not exist, when what actually
/// happened is that it did not occur inside the chosen window.
fn empty_detail(service: &str, fingerprint: &str) -> SpanIssueDetail {
    SpanIssueDetail {
        service: service.to_string(),
        fingerprint: fingerprint.to_string(),
        error_type: String::new(),
        message: String::new(),
        first_seen: 0,
        last_seen: 0,
        occurrences: 0,
        traces: 0,
        operations: 0,
        series: Vec::new(),
        tags: Vec::new(),
        sample_stack: None,
        events: Vec::new(),
    }
}

/// Compose the detail sections over a df already filtered to one `(service, fingerprint)`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn detail_over(
    df: DataFrame,
    promoted: &[String],
    service: &str,
    fingerprint: &str,
    start: i64,
    end: i64,
    sample_n: usize,
) -> Result<SpanIssueDetail, PhotonError> {
    let mut detail = empty_detail(service, fingerprint);
    let attr = |k: &str| get_field(col_ref(span_schema::ATTRIBUTES), k);
    let ts = cast(col_ref(span_schema::START_TIME), DataType::Int64);

    // --- header ---
    let batches = df
        .clone()
        .aggregate(
            vec![],
            vec![
                count(lit(1_i64)).alias("occurrences"),
                count_distinct(col_ref(span_schema::TRACE_ID)).alias("traces"),
                count_distinct(col_ref(span_schema::NAME)).alias("operations"),
                min(ts.clone()).alias("first_seen"),
                max(ts).alias("last_seen"),
                min(attr(ERROR_TYPE)).alias("etype"),
                min(attr(ERROR_MESSAGE)).alias("emsg"),
            ],
        )
        .map_err(|e| PhotonError::Query(format!("span_issue_detail header aggregate: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("span_issue_detail header collect: {e}")))?;

    if let Some(b) = batches.first() {
        if b.num_rows() > 0 {
            let iv = |c: usize| {
                b.column(c)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .map(|a| if a.is_null(0) { 0 } else { a.value(0) })
                    .unwrap_or(0)
            };
            let sv = |c: usize| {
                b.column(c)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .filter(|a| !a.is_null(0))
                    .map(|a| a.value(0).to_string())
                    .unwrap_or_default()
            };
            detail.occurrences = iv(0);
            detail.traces = iv(1);
            detail.operations = iv(2);
            detail.first_seen = iv(3);
            detail.last_seen = iv(4);
            detail.error_type = sv(5);
            detail.message = sv(6);
        }
    }

    // --- occurrences over time: the span histogram's totals, ignoring its status split (every
    // row here is already an error, so the split would be one bar). ---
    detail.series =
        crate::span_histogram::histogram_over(df.clone(), lit(true), start, end, SERIES_BUCKETS)
            .await?
            .into_iter()
            .map(|h| IssueBucket {
                t: h.t,
                count: h.total,
            })
            .collect();

    // --- tag breakdowns ---
    let mut tags = Vec::with_capacity(ISSUE_TAG_FIELDS.len());
    for field in ISSUE_TAG_FIELDS {
        let value = field_col(promoted, field)?;
        let fr = crate::span_facet::facet_over(df.clone(), lit(true), value, TAG_LIMIT).await?;
        tags.push(IssueTag {
            field: field.to_string(),
            values: fr.values,
        });
    }
    detail.tags = tags;

    // --- sample events + a representative stack ---
    let (events, sample_stack) = detail_events(df, promoted, sample_n).await?;
    detail.events = events;
    detail.sample_stack = sample_stack;

    Ok(detail)
}

/// The `n` most-recent spans of `df` as `IssueEvent`s, plus the first `exception.stacktrace`
/// found in their `events` JSON.
async fn detail_events(
    df: DataFrame,
    promoted: &[String],
    n: usize,
) -> Result<(Vec<IssueEvent>, Option<String>), PhotonError> {
    let batches = df
        .select(vec![
            cast(col_ref(span_schema::START_TIME), DataType::Int64).alias("ts"),
            col_ref(span_schema::TRACE_ID).alias("trace"),
            col_ref(span_schema::SPAN_ID).alias("span"),
            col_ref(span_schema::NAME).alias("op"),
            col_ref(span_schema::KIND_TEXT).alias("kind"),
            col_ref(span_schema::DURATION).alias("dur"),
            col_ref(span_schema::STATUS_MESSAGE).alias("smsg"),
            field_col(promoted, "host.name")?.alias("host"),
            field_col(promoted, "deployment.environment")?.alias("env"),
            col_ref(span_schema::EVENTS).alias("events"),
        ])
        .map_err(|e| PhotonError::Query(format!("span_issue_detail events select: {e}")))?
        .sort(vec![
            col("ts").sort(false, false),  // most recent first
            col("span").sort(true, false), // total order, so paging is deterministic
        ])
        .map_err(|e| PhotonError::Query(format!("span_issue_detail events sort: {e}")))?
        .limit(0, Some(n))
        .map_err(|e| PhotonError::Query(format!("span_issue_detail events limit: {e}")))?
        .collect()
        .await
        .map_err(|e| PhotonError::Query(format!("span_issue_detail events collect: {e}")))?;

    let mut events = Vec::new();
    let mut sample_stack: Option<String> = None;
    for b in &batches {
        let s = |c: usize| str_col(b, c, "span_issue_detail events");
        let ts = int_col(b, 0, "span_issue_detail events")?;
        let (trace, span, op, kind) = (s(1)?, s(2)?, s(3)?, s(4)?);
        let dur = int_col(b, 5, "span_issue_detail events")?;
        let (smsg, host, env, raw_events) = (s(6)?, s(7)?, s(8)?, s(9)?);
        for r in 0..b.num_rows() {
            if sample_stack.is_none() && !raw_events.is_null(r) {
                sample_stack = first_stacktrace(raw_events.value(r));
            }
            events.push(IssueEvent {
                timestamp: num(ts, r),
                trace_id: text(trace, r),
                span_id: text(span, r),
                operation: text(op, r),
                kind: text(kind, r),
                duration_nanos: if dur.is_null(r) {
                    None
                } else {
                    Some(dur.value(r))
                },
                status_message: text(smsg, r),
                host: text(host, r),
                environment: text(env, r),
            });
        }
    }
    Ok((events, sample_stack))
}

/// The `exception.stacktrace` of the first `exception` event in a span's stored `events` JSON.
///
/// Tolerant on purpose: a span whose `events` is not the array of objects we wrote is a span from
/// some other writer, and the right answer there is "no stack", never an error that blanks an
/// otherwise complete Issue page.
fn first_stacktrace(events_json: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(events_json).ok()?;
    for ev in parsed.as_array()? {
        if ev.get("name").and_then(|n| n.as_str()) != Some(EXCEPTION_EVENT) {
            continue;
        }
        let stack = ev
            .get("attributes")
            .and_then(|a| a.get(EXCEPTION_STACKTRACE))
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty());
        if let Some(s) = stack {
            return Some(s.to_string());
        }
    }
    None
}

// --- small column helpers, shared by the two readers above ---

fn str_col<'a>(
    b: &'a arrow::record_batch::RecordBatch,
    c: usize,
    who: &str,
) -> Result<&'a StringArray, PhotonError> {
    b.column(c)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| PhotonError::Query(format!("{who}: column {c} not Utf8")))
}

fn int_col<'a>(
    b: &'a arrow::record_batch::RecordBatch,
    c: usize,
    who: &str,
) -> Result<&'a Int64Array, PhotonError> {
    b.column(c)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| PhotonError::Query(format!("{who}: column {c} not Int64")))
}

fn text(a: &StringArray, i: usize) -> String {
    if a.is_null(i) {
        String::new()
    } else {
        a.value(i).to_string()
    }
}

fn opt_text(a: &StringArray, i: usize) -> Option<String> {
    if a.is_null(i) {
        None
    } else {
        Some(a.value(i).to_string())
    }
}

fn num(a: &Int64Array, i: usize) -> i64 {
    if a.is_null(i) {
        0
    } else {
        a.value(i)
    }
}

/// Unused today but kept honest: `Int32Array` is what `status_code`/`kind` decode to, and a future
/// column read here must not silently downcast to the wrong width.
#[allow(dead_code)]
fn i32_col<'a>(
    b: &'a arrow::record_batch::RecordBatch,
    c: usize,
    who: &str,
) -> Result<&'a Int32Array, PhotonError> {
    b.column(c)
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| PhotonError::Query(format!("{who}: column {c} not Int32")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
    use photon_core::span_schema::SpanSchema;

    fn promoted() -> Vec<String> {
        vec![
            "service.name".to_string(),
            "host.name".to_string(),
            "deployment.environment".to_string(),
        ]
    }

    /// One errored span carrying the three attributes Task 14 writes at ingest.
    fn errored(
        service: &str,
        fp: &str,
        trace: &str,
        span_id: &str,
        name: &str,
        ts: i64,
    ) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        attributes.insert("host.name".into(), "node-a".to_string());
        attributes.insert("deployment.environment".into(), "prod".to_string());
        attributes.insert("http.status_code".into(), "500".to_string());
        attributes.insert(ERROR_TYPE.into(), "NullPointerException".to_string());
        attributes.insert(ERROR_MESSAGE.into(), format!("boom {fp}"));
        attributes.insert(ERROR_FINGERPRINT.into(), fp.to_string());
        SpanRecord {
            trace_id: trace.to_string(),
            span_id: span_id.to_string(),
            name: Some(name.to_string()),
            kind: Some(2),
            kind_text: Some("SERVER".into()),
            start_time_nanos: ts,
            duration_nanos: Some(1_000_000),
            status_code: Some(2),
            status_message: Some("internal error".into()),
            events: Some(
                r#"[{"name":"exception","attributes":{"exception.stacktrace":"at f (a.rs:10)"}}]"#
                    .into(),
            ),
            attributes,
            ..Default::default()
        }
    }

    /// A healthy span: no `error.*` attributes at all, which is what `span_error::classify`
    /// returning `None` means on the write side.
    fn healthy(service: &str, ts: i64) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        attributes.insert("host.name".into(), "node-a".to_string());
        attributes.insert("deployment.environment".into(), "prod".to_string());
        SpanRecord {
            trace_id: "t-ok".into(),
            span_id: "s-ok".into(),
            name: Some("GET /health".into()),
            start_time_nanos: ts,
            status_code: Some(1),
            attributes,
            ..Default::default()
        }
    }

    async fn df_of(records: &[SpanRecord]) -> DataFrame {
        let schema = SpanSchema::new(&promoted());
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

    async fn issues(records: &[SpanRecord]) -> Vec<SpanIssue> {
        issues_over(df_of(records).await, lit(true), 50)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn errored_spans_group_by_fingerprint() {
        let rows = issues(&[
            errored("api", "fp1", "t1", "s1", "GET /a", 10),
            errored("api", "fp1", "t2", "s2", "GET /a", 20),
            errored("api", "fp2", "t3", "s3", "GET /b", 30),
        ])
        .await;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].fingerprint, "fp1");
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[1].fingerprint, "fp2");
        assert_eq!(rows[1].count, 1);
    }

    #[tokio::test]
    async fn the_same_exception_in_two_services_is_two_issues() {
        // The whole reason identity is the pair. One hash, two owners.
        let rows = issues(&[
            errored("api", "fp1", "t1", "s1", "GET /a", 10),
            errored("worker", "fp1", "t2", "s2", "consume", 20),
        ])
        .await;
        assert_eq!(rows.len(), 2);
        let services: Vec<&str> = rows.iter().map(|r| r.service.as_str()).collect();
        assert!(services.contains(&"api"));
        assert!(services.contains(&"worker"));
        assert!(rows.iter().all(|r| r.fingerprint == "fp1"));
    }

    #[tokio::test]
    async fn a_healthy_span_is_in_no_issue() {
        let rows = issues(&[
            healthy("api", 5),
            healthy("api", 6),
            errored("api", "fp1", "t1", "s1", "GET /a", 10),
        ])
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].count, 1);
    }

    #[tokio::test]
    async fn traces_counts_requests_not_spans() {
        // Two errored spans in ONE trace is one broken request, not two.
        let rows = issues(&[
            errored("api", "fp1", "t1", "s1", "GET /a", 10),
            errored("api", "fp1", "t1", "s2", "GET /a", 11),
        ])
        .await;
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].traces, 1);
    }

    #[tokio::test]
    async fn operations_counts_the_distinct_names_one_issue_reaches() {
        let rows = issues(&[
            errored("api", "fp1", "t1", "s1", "GET /a", 10),
            errored("api", "fp1", "t2", "s2", "GET /b", 20),
        ])
        .await;
        assert_eq!(rows[0].operations, 2);
    }

    #[tokio::test]
    async fn first_and_last_seen_span_the_group() {
        let rows = issues(&[
            errored("api", "fp1", "t1", "s1", "GET /a", 10),
            errored("api", "fp1", "t2", "s2", "GET /a", 900),
        ])
        .await;
        assert_eq!(rows[0].first_seen, 10);
        assert_eq!(rows[0].last_seen, 900);
    }

    #[tokio::test]
    async fn an_empty_window_yields_no_issues() {
        assert!(issues(&[]).await.is_empty());
    }

    #[tokio::test]
    async fn the_detail_reads_its_stack_out_of_the_events_json() {
        let records = [errored("api", "fp1", "t1", "s1", "GET /a", 10)];
        let d = detail_over(
            df_of(&records).await,
            &promoted(),
            "api",
            "fp1",
            0,
            1_000,
            20,
        )
        .await
        .unwrap();
        assert_eq!(d.occurrences, 1);
        assert_eq!(d.error_type, "NullPointerException");
        assert_eq!(d.sample_stack.as_deref(), Some("at f (a.rs:10)"));
        assert_eq!(d.events.len(), 1);
        assert_eq!(d.events[0].operation, "GET /a");
        assert_eq!(d.events[0].host, "node-a");
        assert_eq!(d.events[0].environment, "prod");
    }

    #[tokio::test]
    async fn the_detail_breaks_down_promoted_and_long_tail_tags_alike() {
        let records = [
            errored("api", "fp1", "t1", "s1", "GET /a", 10),
            errored("api", "fp1", "t2", "s2", "GET /b", 20),
        ];
        let d = detail_over(
            df_of(&records).await,
            &promoted(),
            "api",
            "fp1",
            0,
            1_000,
            20,
        )
        .await
        .unwrap();
        let by = |f: &str| {
            d.tags
                .iter()
                .find(|t| t.field == f)
                .unwrap_or_else(|| panic!("no tag {f}"))
        };
        // `name` is a fixed column, `deployment.environment` promoted, `http.status_code` long-tail.
        assert_eq!(by("name").values.len(), 2);
        assert_eq!(by("deployment.environment").values[0].value, "prod");
        assert_eq!(by("http.status_code").values[0].value, "500");
    }

    #[tokio::test]
    async fn the_detail_series_covers_the_whole_window() {
        let records = [errored("api", "fp1", "t1", "s1", "GET /a", 10)];
        let d = detail_over(
            df_of(&records).await,
            &promoted(),
            "api",
            "fp1",
            0,
            1_000,
            20,
        )
        .await
        .unwrap();
        assert_eq!(d.series.len(), SERIES_BUCKETS);
        assert_eq!(d.series.iter().map(|b| b.count).sum::<u64>(), 1);
    }

    #[test]
    fn a_span_with_no_exception_event_has_no_stack_rather_than_a_wrong_one() {
        assert_eq!(first_stacktrace(r#"[{"name":"log"}]"#), None);
        assert_eq!(first_stacktrace("not json"), None);
        assert_eq!(first_stacktrace("[]"), None);
        assert_eq!(
            first_stacktrace(r#"[{"name":"exception","attributes":{}}]"#),
            None
        );
    }

    #[test]
    fn the_stack_comes_from_the_first_exception_event_that_has_one() {
        let json = r#"[
            {"name":"exception","attributes":{"exception.type":"E"}},
            {"name":"exception","attributes":{"exception.stacktrace":"at g (b.rs:2)"}}
        ]"#;
        assert_eq!(first_stacktrace(json).as_deref(), Some("at g (b.rs:2)"));
    }
}
