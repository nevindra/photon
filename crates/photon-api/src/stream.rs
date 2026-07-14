//! SSE live-tail streaming: `GET /api/stream/logs` and `GET /api/stream/spans`.
//!
//! Two Server-Sent-Events endpoints that subscribe to the broadcast of appended WAL batches (fed
//! by `photon_wal::BroadcastingWal`, wired into the write path by BE-5), filter each row
//! server-side with the **same** query grammar `/api/search` uses (`resolve_query` /
//! `resolve_span_query` → `ResolvedQuery::matches` / `SpanResolvedQuery::matches`), and push
//! surviving rows to the browser on a coalescing flush.
//!
//! The stream sits **off** the ack path and **off** the query path — it is a best-effort, lossy
//! view. A client that falls behind the broadcast ring sees an `event: lag` (never a silent
//! drop); a flush that overflows `max_rows_per_flush` keeps the newest rows and reports the true
//! match rate via `event: rate`.
//!
//! Events emitted:
//! - `event: rows`  — a JSON array of matched rows, **oldest-first** (the UI prepends+reverses).
//! - `event: lag`   — `{ "skipped": n }` when the subscriber lagged the broadcast ring.
//! - `event: rate`  — `{ "matched_per_sec": n }` when a flush was truncated to the row cap.
//! - `: ping`       — keepalive comment (axum `KeepAlive`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arrow::record_batch::RecordBatch;
use axum::extract::{Query as AxumQuery, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use tokio::sync::{broadcast, OwnedSemaphorePermit, Semaphore};

use photon_core::config::LiveConfig;
use photon_core::query::{ResolvedQuery, SpanResolvedQuery};
use photon_core::record::log_record_from_batch;
use photon_core::span_record::span_record_from_batch;

use crate::query_params::{resolve_query, resolve_span_query};
use crate::search::row_to_json;
use crate::traces::span_row_to_json;
use crate::AppState;

/// Shared server-push fan-out: the two broadcast senders that appended batches arrive on, the
/// live-tail tuning knobs, and the global connection semaphore.
///
/// Built and attached by `photon-server` via [`crate::ApiServer::with_live_hub`]; when absent
/// (`None` on the app state) the two stream routes return 404, mirroring the other optional
/// subsystems (`with_uptime` / `with_data_admin`).
pub struct LiveHub {
    /// Appended log batches, broadcast by `BroadcastingWal` after the WAL `fsync` acks.
    pub logs: broadcast::Sender<Arc<RecordBatch>>,
    /// Appended span batches, broadcast by `BroadcastingWal` after the WAL `fsync` acks.
    pub spans: broadcast::Sender<Arc<RecordBatch>>,
    /// Live-tail tuning (flush cadence, per-flush row cap, connection cap).
    pub cfg: LiveConfig,
    /// Global cap on concurrent SSE connections across **both** endpoints; a connect that can't
    /// acquire a permit is rejected 503. A permit is held for the connection's lifetime.
    pub conns: Arc<Semaphore>,
}

impl LiveHub {
    /// Build a hub from the two broadcast senders and the live config, sizing the connection
    /// semaphore from `cfg.max_connections`. (Fields stay public so a struct literal works too.)
    pub fn new(
        logs: broadcast::Sender<Arc<RecordBatch>>,
        spans: broadcast::Sender<Arc<RecordBatch>>,
        cfg: LiveConfig,
    ) -> LiveHub {
        let conns = Arc::new(Semaphore::new(cfg.max_connections));
        LiveHub {
            logs,
            spans,
            cfg,
            conns,
        }
    }
}

/// Decode every row of `batch`, keep the ones `q` matches, and serialize each survivor with the
/// shared [`row_to_json`] — assigning a **connection-monotonic** `id` that advances only for
/// surviving rows (a stable render key that never resets across flushes). Rows are appended in
/// batch (arrival) order, i.e. **oldest-first**, which is the order the flush payload preserves.
pub(crate) fn filter_log_batch(
    batch: &RecordBatch,
    q: &ResolvedQuery,
    next_id: &mut i64,
) -> Vec<Value> {
    let mut out = Vec::new();
    for row in 0..batch.num_rows() {
        // `log_record_from_batch` folds `service.name` (and every promoted column) into
        // `attributes`, so the resolved predicate matches exactly as it does over Parquet.
        let rec = log_record_from_batch(batch, row);
        if q.matches(&rec) {
            out.push(row_to_json(batch, row, *next_id));
            *next_id += 1;
        }
    }
    out
}

/// Spans sibling of [`filter_log_batch`]: decode → [`SpanResolvedQuery::matches`] →
/// [`span_row_to_json`], with the same connection-monotonic, oldest-first id semantics.
pub(crate) fn filter_span_batch(
    batch: &RecordBatch,
    q: &SpanResolvedQuery,
    next_id: &mut i64,
) -> Vec<Value> {
    let mut out = Vec::new();
    for row in 0..batch.num_rows() {
        let rec = span_record_from_batch(batch, row);
        if q.matches(&rec) {
            out.push(span_row_to_json(batch, row, *next_id));
            *next_id += 1;
        }
    }
    out
}

/// Scale a per-flush-window match count into an approximate matches-per-second rate. The client
/// renders `matched_per_sec` as "…/s", so the raw per-window count would read low by the ratio of
/// the flush window to one second (e.g. ~4x low at the 250ms default).
fn per_second_rate(matched: usize, flush: Duration) -> u64 {
    let ms = flush.as_millis().max(1) as u64;
    (matched as u64) * 1000 / ms
}

/// Assemble the coalescing SSE response body shared by both endpoints.
///
/// Subscribes to `rx` and runs a `select!` loop: each received batch is filtered (`filter`) and
/// buffered in arrival order; on every `flush` tick a non-empty buffer is emitted as one
/// `event: rows` (capped at `cap`, keeping the newest and emitting `event: rate` when truncated).
/// A `Lagged(n)` recv surfaces as `event: lag`; `Closed` ends the stream. `permit` is moved into
/// the body so the connection slot is released only when the stream is dropped (client disconnect
/// or server shutdown).
fn sse_response<F>(
    mut rx: broadcast::Receiver<Arc<RecordBatch>>,
    flush: Duration,
    cap: usize,
    permit: OwnedSemaphorePermit,
    mut filter: F,
) -> Response
where
    F: FnMut(&RecordBatch, &mut i64) -> Vec<Value> + Send + 'static,
{
    let body = async_stream::stream! {
        // Held for the connection lifetime; dropped (releasing the slot) when the stream ends.
        let _permit = permit;
        let mut next_id: i64 = 0;
        let mut buf: Vec<Value> = Vec::new();
        let mut matched_since_flush: usize = 0;
        let mut ticker = tokio::time::interval(flush);
        loop {
            tokio::select! {
                recv = rx.recv() => match recv {
                    Ok(batch) => {
                        let rows = filter(batch.as_ref(), &mut next_id);
                        matched_since_flush += rows.len();
                        buf.extend(rows);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        yield Ok::<Event, std::convert::Infallible>(
                            Event::default()
                                .event("lag")
                                .data(json!({ "skipped": n }).to_string()),
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = ticker.tick() => {
                    if !buf.is_empty() {
                        // Keep the newest `cap` rows; report the observed match rate iff we truncated.
                        if buf.len() > cap {
                            let rate = per_second_rate(matched_since_flush, flush);
                            buf.drain(0..buf.len() - cap);
                            yield Ok(Event::default()
                                .event("rate")
                                .data(json!({ "matched_per_sec": rate }).to_string()));
                        }
                        // Emit in arrival (oldest-first) order — the frontend prepends+reverses.
                        let payload = std::mem::take(&mut buf);
                        yield Ok(Event::default()
                            .event("rows")
                            .data(Value::Array(payload).to_string()));
                        matched_since_flush = 0;
                    }
                }
            }
        }
    };

    Sse::new(body)
        .keep_alive(KeepAlive::default().text("ping"))
        .into_response()
}

/// `GET /api/stream/logs?q=<grammar>` — live-tail matching log rows over SSE.
///
/// Resolves `q` against the log schema's promoted attributes via the same [`resolve_query`] path
/// `/api/search` uses (empty/blank ⇒ match-all); a parse/resolve error is a `400` with the shared
/// `{ error, offset? }` body and no stream is opened. 404 when live streaming is disabled, 503
/// when the connection cap is saturated.
pub(crate) async fn stream_logs(
    State(state): State<AppState>,
    AxumQuery(params): AxumQuery<HashMap<String, String>>,
) -> Response {
    let hub = match state.live.clone() {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, "live streaming disabled").into_response(),
    };

    // Parse + resolve up front; a bad query is a 400, never an opened-then-failing stream.
    let q_text = params.get("q").map(String::as_str).unwrap_or("");
    let resolved = match resolve_query(q_text, state.query.promoted_attributes()) {
        Ok(r) => r.unwrap_or_default(), // empty grammar ⇒ empty ResolvedQuery ⇒ match every row
        Err(e) => return e.into_response(),
    };

    let permit = match hub.conns.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "too many live streams").into_response()
        }
    };

    let rx = hub.logs.subscribe();
    let flush = Duration::from_millis(hub.cfg.flush_interval_ms.max(1));
    let cap = hub.cfg.max_rows_per_flush.max(1);
    sse_response(rx, flush, cap, permit, move |batch, next_id| {
        filter_log_batch(batch, &resolved, next_id)
    })
}

/// `GET /api/stream/spans?q=<grammar>` — live-tail matching span rows over SSE. The spans sibling
/// of [`stream_logs`]: same auth, hub-gating, 400/503 semantics, resolving against the spans
/// schema ([`resolve_span_query`]) and serializing with [`span_row_to_json`].
pub(crate) async fn stream_spans(
    State(state): State<AppState>,
    AxumQuery(params): AxumQuery<HashMap<String, String>>,
) -> Response {
    let hub = match state.live.clone() {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, "live streaming disabled").into_response(),
    };

    let q_text = params.get("q").map(String::as_str).unwrap_or("");
    let resolved = match resolve_span_query(q_text, state.span_query.promoted_attributes()) {
        Ok(r) => r.unwrap_or_default(),
        Err(e) => return e.into_response(),
    };

    let permit = match hub.conns.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "too many live streams").into_response()
        }
    };

    let rx = hub.spans.subscribe();
    let flush = Duration::from_millis(hub.cfg.flush_interval_ms.max(1));
    let cap = hub.cfg.max_rows_per_flush.max(1);
    sse_response(rx, flush, cap, permit, move |batch, next_id| {
        filter_span_batch(batch, &resolved, next_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_core::query::{parse, FieldResolver};
    use photon_core::record::{LogRecord, RecordBatchBuilder};
    use photon_core::schema::LogSchema;
    use std::collections::BTreeMap;

    fn batch_with_service(svc: &str) -> arrow::record_batch::RecordBatch {
        let schema = LogSchema::new(&["service.name".to_string()]);
        let mut b = RecordBatchBuilder::with_capacity(&schema, 1);
        let mut a = BTreeMap::new();
        a.insert("service.name".to_string(), svc.to_string());
        b.append(&LogRecord {
            timestamp_nanos: 1,
            body: Some("hi".into()),
            attributes: a,
            ..Default::default()
        });
        b.finish().unwrap()
    }

    #[test]
    fn filters_batch_rows_by_query_and_serializes() {
        let resolver = FieldResolver::new(&["service.name".to_string()]);
        let q = resolver.resolve(&parse("service:keep").unwrap()).unwrap();
        let mut next_id: i64 = 100;

        let kept = filter_log_batch(&batch_with_service("keep"), &q, &mut next_id);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["service"], "keep");
        assert_eq!(kept[0]["id"], 100);
        assert_eq!(
            next_id, 101,
            "monotonic id advances only for surviving rows"
        );

        let dropped = filter_log_batch(&batch_with_service("other"), &q, &mut next_id);
        assert!(dropped.is_empty());
        assert_eq!(next_id, 101, "non-matching rows do not consume ids");
    }

    #[test]
    fn per_second_rate_scales_window_count_to_per_second() {
        // 200 matches in a 250ms window ≈ 800/s (not the raw per-window 200).
        assert_eq!(per_second_rate(200, Duration::from_millis(250)), 800);
        // A 1s window is unscaled.
        assert_eq!(per_second_rate(50, Duration::from_millis(1000)), 50);
        // A degenerate 0ms flush is clamped to 1ms — never divides by zero.
        assert_eq!(per_second_rate(3, Duration::from_millis(0)), 3000);
    }

    // ---- Route wiring (auth-gate / hub-gate / 400-before-stream). The full socket loop is
    // covered by the BE-6 e2e; these one-shot requests all return *before* a stream is opened,
    // so none of them hang. ----

    use tower::ServiceExt; // for `oneshot`

    /// A routed server WITH a live hub attached (so the stream routes are enabled). Uses throwaway
    /// broadcast channels — no batch is ever sent, so only the pre-stream paths are exercised.
    fn router_with_hub() -> axum::Router {
        let (logs, _) = broadcast::channel(16);
        let (spans, _) = broadcast::channel(16);
        let hub = LiveHub::new(logs, spans, LiveConfig::default());
        crate::test_server().with_live_hub(hub).into_router()
    }

    async fn get_status(router: axum::Router, uri: &str, cookie: Option<&str>) -> StatusCode {
        let mut req = axum::http::Request::builder().method("GET").uri(uri);
        if let Some(c) = cookie {
            req = req.header(axum::http::header::COOKIE, c);
        }
        router
            .oneshot(req.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn stream_logs_requires_session() {
        // No cookie ⇒ the shared session-auth layer rejects before the handler runs.
        let status = get_status(router_with_hub(), "/api/stream/logs", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn stream_spans_requires_session() {
        let status = get_status(router_with_hub(), "/api/stream/spans", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn stream_404_when_hub_absent() {
        // `test_router()` has no live hub attached ⇒ authed request 404s (mirrors uptime/data).
        let router = crate::test_router();
        let cookie = crate::session_cookie(&router).await;
        assert_eq!(
            get_status(router, "/api/stream/logs", Some(&cookie)).await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn bad_query_is_400_before_opening_stream() {
        // A malformed grammar is a 400 with a byte offset — never an opened-then-failing stream.
        let router = router_with_hub();
        let cookie = crate::session_cookie(&router).await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/api/stream/logs?q=ok%20:bad")
                    .header(axum::http::header::COOKIE, cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["offset"], json!(3));
    }
}
