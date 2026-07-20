//! Implements the alerts `ConditionSource` seam over the three read engines. Lives in
//! photon-server — the only crate allowed to depend on both `photon-alerts` and `photon-query` —
//! so `photon-alerts` stays free of any dependency on the query layer.
//!
//! Each `sample_*` method translates one condition into the request the matching `photon-api`
//! handler builds (`metrics.rs`, `red.rs`, `search.rs`, `rum.rs`) and reduces the engine's result
//! to one scalar per evaluated series. `Ok(vec![])` means "nothing matched/crossed" (drives
//! resolves); `Err` means "could not evaluate this tick" (state left unchanged).
//!
//! Wave-W3, Task 6. The engines derive `Clone` (Arc-backed), so this source holds owned clones —
//! T8 constructs it from clones of the same engines it hands to `ApiServer::new`.

// The struct is constructed + wired into the scheduler in T8; until then a non-test binary build
// has no caller for it, so allow dead_code at the module level (the test below exercises it).
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;

use photon_alerts::model::{
    Condition, LogCondition, MetricAgg, MetricCondition, RumCondition, RumKind, SeriesSample,
    TraceCondition, TraceKind,
};
use photon_alerts::source::ConditionSource;
use photon_core::config::DEFAULT_APDEX_THRESHOLD_MS;
use photon_core::metric_agg::Agg;
use photon_core::query::{
    parse, FieldResolver, MetricFieldResolver, MetricResolvedKind, MetricResolvedQuery,
    MetricResolvedTerm, ResolvedQuery,
};
use photon_core::PhotonError;
use photon_query::{
    MetricSeriesRequest, MetricsQueryEngine, QueryEngine, QueryRequest, RedGroup, SpanQueryEngine,
    SpanQueryRequest, SpanSort,
};

/// One nanosecond bucket for the metrics/RED reductions — the alert needs a single aggregate over
/// the whole window, not a chart, so one bucket over `[start, now]` yields exactly one point per
/// series whose value is that window aggregate.
const SINGLE_BUCKET: usize = 1;

/// Cap on distinct RUM error fingerprints summed for `error_count`. `rum_errors` returns the top-N
/// fingerprint groups by occurrence; summing them approximates the window's total error count.
/// Real apps carry far fewer than this many distinct fingerprints in one window.
const RUM_ERROR_FINGERPRINT_LIMIT: usize = 10_000;

/// The alerts `ConditionSource` implemented over the three query engines. All three derive `Clone`
/// (Arc-backed manifest/services caches), so owning them by value is cheap — a clone shares the
/// same caches with the copy handed to `ApiServer`.
pub struct EngineConditionSource {
    pub logs: QueryEngine,
    pub spans: SpanQueryEngine,
    pub metrics: MetricsQueryEngine,
}

impl EngineConditionSource {
    pub fn new(logs: QueryEngine, spans: SpanQueryEngine, metrics: MetricsQueryEngine) -> Self {
        Self {
            logs,
            spans,
            metrics,
        }
    }
}

#[async_trait]
impl ConditionSource for EngineConditionSource {
    async fn sample(
        &self,
        cond: &Condition,
        now_ms: i64,
    ) -> Result<Vec<SeriesSample>, PhotonError> {
        let now_ns = now_ms.saturating_mul(1_000_000);
        match cond {
            Condition::Metrics(c) => self.sample_metrics(c, now_ns).await,
            Condition::Logs(c) => self.sample_logs(c, now_ns).await,
            Condition::Traces(c) => self.sample_traces(c, now_ns).await,
            Condition::Rum(c) => self.sample_rum(c, now_ns).await,
        }
    }
}

/// Window start = `now - window_secs` (nanoseconds), saturating so an absurd `window_secs` can
/// never overflow into a start *after* `now`.
fn window_start(now_ns: i64, window_secs: i64) -> i64 {
    now_ns.saturating_sub(window_secs.saturating_mul(1_000_000_000))
}

/// Window length in seconds, floored at 1ns → a tiny positive number, so a rate denominator is
/// never zero (mirrors `photon-api`'s `red.rs::window_seconds`).
fn window_seconds(start_ns: i64, end_ns: i64) -> f64 {
    (end_ns - start_ns).max(1) as f64 / 1_000_000_000.0
}

/// Map an alert `MetricAgg` to the query engine's `Agg`. `P95` has no `Agg` counterpart — the
/// engine only reassembles p50/p90/p99 quantiles — so it is an explicit, honest error rather than
/// a silent approximation.
fn map_metric_agg(agg: MetricAgg) -> Result<Agg, PhotonError> {
    Ok(match agg {
        MetricAgg::Avg => Agg::Avg,
        MetricAgg::Min => Agg::Min,
        MetricAgg::Max => Agg::Max,
        MetricAgg::Sum => Agg::Sum,
        MetricAgg::Last => Agg::Last,
        MetricAgg::P50 => Agg::P50,
        MetricAgg::P90 => Agg::P90,
        MetricAgg::P99 => Agg::P99,
        MetricAgg::Rate => Agg::Rate,
        MetricAgg::Increase => Agg::Increase,
        MetricAgg::P95 => {
            return Err(PhotonError::Query(
                "alert metric aggregation `p95` is unsupported (the metrics engine reassembles \
                 only p50/p90/p99)"
                    .to_string(),
            ))
        }
    })
}

impl EngineConditionSource {
    /// Metrics: build the same `MetricSeriesRequest` as `photon-api`'s `metrics.rs::query` (one
    /// bucket over the window, the mapped agg, the label filter + group-by), then take each
    /// series' single window-aggregate point. `key` carries the group-by labels.
    async fn sample_metrics(
        &self,
        c: &MetricCondition,
        now_ns: i64,
    ) -> Result<Vec<SeriesSample>, PhotonError> {
        let start = window_start(now_ns, c.window_secs);
        let agg = map_metric_agg(c.agg)?;
        let filter = self.build_metric_filter(&c.label_filters)?;

        let req = MetricSeriesRequest {
            metric: c.metric_name.clone(),
            agg: Some(agg),
            group_by: c.group_by.clone(),
            filter,
            start_ts_nanos: start,
            end_ts_nanos: now_ns,
            buckets: SINGLE_BUCKET,
        };
        let result = self.metrics.query_series(req).await?;

        let mut out = Vec::with_capacity(result.series.len());
        for s in result.series {
            // One bucket ⇒ one point; its value is the window aggregate. A gap (None) means the
            // series had no data in the window, so it produces no sample.
            let value = s.points.last().and_then(|p| p.v);
            if let Some(v) = value {
                let key: Vec<(String, String)> = s.labels.into_iter().collect();
                out.push(SeriesSample { key, value: v });
            }
        }
        Ok(out)
    }

    /// Compile the structured `label_filters` map directly into a `MetricResolvedQuery` via the
    /// metrics field resolver — the same resolver `metrics.rs` reaches through `parse`, but built
    /// term-by-term so no query string needs escaping. Empty map ⇒ no filter.
    fn build_metric_filter(
        &self,
        label_filters: &BTreeMap<String, String>,
    ) -> Result<Option<MetricResolvedQuery>, PhotonError> {
        if label_filters.is_empty() {
            return Ok(None);
        }
        let resolver = MetricFieldResolver::new(self.metrics.promoted_attributes());
        let mut terms = Vec::with_capacity(label_filters.len());
        for (k, v) in label_filters {
            let field = resolver.resolve_field_name(k).map_err(|e| {
                PhotonError::Query(format!("alert metric label filter `{k}`: {}", e.message))
            })?;
            terms.push(MetricResolvedTerm {
                negated: false,
                kind: MetricResolvedKind::Match {
                    field,
                    values: vec![v.clone()],
                },
            });
        }
        Ok(Some(MetricResolvedQuery { terms }))
    }

    /// Logs: a `COUNT(*)` over the pruned match set via `count_matching`, built like
    /// `search.rs`/`build_query_request`. Ungrouped ⇒ one aggregate series (emitted even at 0, so
    /// a `count < N` alert can trigger and a `count > N` alert can resolve). Grouped by
    /// `service.name` ⇒ one count per distinct service, emitting only services with matches in the
    /// window (an absent service disappears from the set and resolves).
    async fn sample_logs(
        &self,
        c: &LogCondition,
        now_ns: i64,
    ) -> Result<Vec<SeriesSample>, PhotonError> {
        let start = window_start(now_ns, c.window_secs);
        let resolved = self.resolve_log_query(&c.query)?;

        match c.group_by.as_deref() {
            None => {
                let req = log_count_request(start, now_ns, Vec::new(), resolved);
                let n = self.logs.count_matching(req).await?;
                Ok(vec![SeriesSample {
                    key: Vec::new(),
                    value: n as f64,
                }])
            }
            Some("service.name") | Some("service") => {
                let services = self.logs.distinct_services().await?;
                let mut out = Vec::new();
                for svc in services.iter() {
                    let req = log_count_request(start, now_ns, vec![svc.clone()], resolved.clone());
                    let n = self.logs.count_matching(req).await?;
                    if n > 0 {
                        out.push(SeriesSample {
                            key: vec![("service.name".to_string(), svc.clone())],
                            value: n as f64,
                        });
                    }
                }
                Ok(out)
            }
            Some(other) => Err(PhotonError::Query(format!(
                "alert log group_by `{other}` unsupported (only `service.name`)"
            ))),
        }
    }

    /// Parse+resolve a log-grammar filter string against the logs schema, exactly as
    /// `photon-api`'s `query_params::resolve_query` does. Blank ⇒ `None`.
    fn resolve_log_query(&self, query: &str) -> Result<Option<ResolvedQuery>, PhotonError> {
        if query.trim().is_empty() {
            return Ok(None);
        }
        let ast = parse(query)
            .map_err(|e| PhotonError::Query(format!("alert log query parse: {}", e.message)))?;
        let resolved = FieldResolver::new(self.logs.promoted_attributes())
            .resolve(&ast)
            .map_err(|e| PhotonError::Query(format!("alert log query resolve: {}", e.message)))?;
        Ok(Some(resolved))
    }

    /// Traces: one `red_metrics` pass grouped per service (or per service+operation when an
    /// operation is pinned) — the RED row carries `count`, `error_count`, and the p50/p90/p99
    /// (t-digest) all at once, so a single call feeds every `TraceKind`. Picks the row for
    /// `c.service` and derives the scalar; error_rate/request_rate mirror `photon-api`'s
    /// `red.rs`. No matching row ⇒ `Ok(vec![])` (resolve). `key` = `[("service.name", svc)]`.
    async fn sample_traces(
        &self,
        c: &TraceCondition,
        now_ns: i64,
    ) -> Result<Vec<SeriesSample>, PhotonError> {
        let start = window_start(now_ns, c.window_secs);
        let req = SpanQueryRequest {
            start_ts_nanos: start,
            end_ts_nanos: now_ns,
            query: None,
            sort: SpanSort::Recent,
            limit: 0,
            offset: 0,
            projected_attributes: Vec::new(),
        };
        // Grouping per operation when one is pinned lets us select the exact (service, operation)
        // row; otherwise a per-service rollup. Apdex thresholds only affect the satisfied/
        // tolerating/frustrated bands, which alerts never read, so an empty map + the default is
        // sufficient here (the settings store isn't reachable from this seam).
        let group = if c.operation.is_some() {
            RedGroup::Operation
        } else {
            RedGroup::Service
        };
        let rows = self
            .spans
            .red_metrics(req, group, &HashMap::new(), DEFAULT_APDEX_THRESHOLD_MS)
            .await?;

        let row = rows.iter().find(|r| {
            r.service == c.service
                && match &c.operation {
                    Some(op) => r.operation.as_deref() == Some(op.as_str()),
                    None => true,
                }
        });
        let Some(row) = row else {
            return Ok(Vec::new());
        };

        let value = match c.kind {
            TraceKind::ErrorRate => {
                // The UI enters this threshold as a percentage (`5` = 5%, with a `%` unit label),
                // so return the rate as a 0–100 percentage — NOT a 0–1 fraction — or "> 5%" would
                // compare `0.05 > 5.0` and never fire. (Guard divide-by-zero: 0 count ⇒ 0.0.)
                if row.count > 0 {
                    row.error_count as f64 / row.count as f64 * 100.0
                } else {
                    0.0
                }
            }
            TraceKind::RequestRate => row.count as f64 / window_seconds(start, now_ns),
            // Latency percentiles are the RED row's t-digest p50/p90/p99 (nanoseconds); the same
            // approximation `SpanQueryEngine::latency` uses. `LatencyP95` has no p95 column.
            TraceKind::LatencyP50 => row.p50 as f64,
            TraceKind::LatencyP90 => row.p90 as f64,
            TraceKind::LatencyP99 => row.p99 as f64,
            TraceKind::LatencyP95 => {
                return Err(PhotonError::Query(
                    "alert trace latency `p95` is unsupported (RED/latency expose only \
                     p50/p90/p99)"
                        .to_string(),
                ))
            }
        };

        Ok(vec![SeriesSample {
            key: vec![("service.name".to_string(), c.service.clone())],
            value,
        }])
    }

    /// RUM: Web-Vitals kinds take the requested vital's p75 from `rum_vitals` (app-scoped — the
    /// engine has no per-route vitals path, so `c.route` is only honored for errors). `ErrorCount`
    /// sums the occurrence counts from `rum_errors` (route-scoped when set). `service` = `app_id`.
    async fn sample_rum(
        &self,
        c: &RumCondition,
        now_ns: i64,
    ) -> Result<Vec<SeriesSample>, PhotonError> {
        let start = window_start(now_ns, c.window_secs);

        match c.kind {
            RumKind::ErrorCount => {
                let issues = self
                    .logs
                    .rum_errors(
                        &c.app_id,
                        start,
                        now_ns,
                        RUM_ERROR_FINGERPRINT_LIMIT,
                        c.route.as_deref(),
                        None,
                    )
                    .await?;
                let total: i64 = issues.iter().map(|i| i.count).sum();
                Ok(vec![SeriesSample {
                    key: Vec::new(),
                    value: total as f64,
                }])
            }
            RumKind::VitalLcpP75
            | RumKind::VitalInpP75
            | RumKind::VitalClsP75
            | RumKind::VitalFcpP75
            | RumKind::VitalTtfbP75 => {
                let metric = vital_metric_name(c.kind);
                let vitals = self.metrics.rum_vitals(&c.app_id, start, now_ns).await?;
                match vitals.iter().find(|v| v.metric == metric) {
                    Some(v) => Ok(vec![SeriesSample {
                        key: Vec::new(),
                        value: v.p75,
                    }]),
                    // No samples for this vital in the window ⇒ nothing to evaluate (resolve).
                    None => Ok(Vec::new()),
                }
            }
        }
    }
}

/// Build the logs `count_matching` request (mirrors `build_query_request`: no severity/text,
/// `limit = 0` — the count ignores the row limit).
fn log_count_request(
    start_ts_nanos: i64,
    end_ts_nanos: i64,
    services: Vec<String>,
    query: Option<ResolvedQuery>,
) -> QueryRequest {
    QueryRequest {
        start_ts_nanos,
        end_ts_nanos,
        services,
        severities: Vec::new(),
        text: None,
        query,
        limit: 0,
    }
}

/// The `web_vitals.*` metric name backing a RUM vital kind (only the vital kinds reach here).
fn vital_metric_name(kind: RumKind) -> &'static str {
    match kind {
        RumKind::VitalLcpP75 => "web_vitals.lcp",
        RumKind::VitalInpP75 => "web_vitals.inp",
        RumKind::VitalClsP75 => "web_vitals.cls",
        RumKind::VitalFcpP75 => "web_vitals.fcp",
        RumKind::VitalTtfbP75 => "web_vitals.ttfb",
        RumKind::ErrorCount => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_alerts::model::Cmp;
    use photon_core::metric_schema::MetricSchema;
    use photon_core::schema::LogSchema;
    use photon_core::span_schema::SpanSchema;

    /// Three engines over an empty (freshly-created, no compacted data) hot dir. Every query path
    /// short-circuits on an empty manifest, so this exercises request construction, the window
    /// math, and the result reduction without any compaction.
    fn empty_source() -> EngineConditionSource {
        let dir = tempfile::tempdir().unwrap();
        let hot = dir.path().to_path_buf();
        // Keep the tempdir alive for the engines' lifetime by leaking it — this is a unit test.
        std::mem::forget(dir);
        let logs =
            QueryEngine::new(hot.clone(), LogSchema::new(&["service.name".to_string()])).unwrap();
        let spans =
            SpanQueryEngine::new(hot.clone(), SpanSchema::new(&["service.name".to_string()]))
                .unwrap();
        let metrics =
            MetricsQueryEngine::new(hot, MetricSchema::new(&["service.name".to_string()])).unwrap();
        EngineConditionSource::new(logs, spans, metrics)
    }

    fn metric_cond(agg: MetricAgg, group_by: Vec<String>) -> Condition {
        Condition::Metrics(MetricCondition {
            metric_name: "system.cpu.utilization".to_string(),
            label_filters: BTreeMap::new(),
            group_by,
            agg,
            window_secs: 300,
            cmp: Cmp::Gt,
            threshold: 0.9,
        })
    }

    #[tokio::test]
    async fn metrics_over_empty_store_is_no_series() {
        let src = empty_source();
        let out = src
            .sample(&metric_cond(MetricAgg::Avg, vec![]), 1_700_000_000_000)
            .await
            .unwrap();
        assert!(out.is_empty(), "no data ⇒ no series, got {out:?}");
    }

    #[tokio::test]
    async fn metric_p95_is_an_explicit_error() {
        let src = empty_source();
        let err = src
            .sample(&metric_cond(MetricAgg::P95, vec![]), 1_700_000_000_000)
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("p95"),
            "expected a p95 error, got {err}"
        );
    }

    #[tokio::test]
    async fn ungrouped_log_count_emits_one_zero_series() {
        let src = empty_source();
        let cond = Condition::Logs(LogCondition {
            query: "level:error".to_string(),
            group_by: None,
            window_secs: 300,
            cmp: Cmp::Gt,
            threshold: 10.0,
        });
        let out = src.sample(&cond, 1_700_000_000_000).await.unwrap();
        // An aggregate count always yields exactly one series (0 over an empty store) so that a
        // `count < N` alert can trigger and a `count > N` alert can resolve.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value, 0.0);
        assert!(out[0].key.is_empty());
    }

    #[tokio::test]
    async fn rum_vital_over_empty_store_is_no_series() {
        let src = empty_source();
        let cond = Condition::Rum(RumCondition {
            app_id: "web".to_string(),
            route: None,
            kind: RumKind::VitalLcpP75,
            window_secs: 300,
            cmp: Cmp::Gt,
            threshold: 2500.0,
        });
        let out = src.sample(&cond, 1_700_000_000_000).await.unwrap();
        assert!(out.is_empty());
    }
}
