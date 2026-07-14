//! photon-ingest: OTLP logs receivers (gRPC + HTTP), token auth, OTLP -> LogRecord mapping,
//! WAL append.
//!
//! Implemented per the `photon-ingest` section of
//! `docs/superpowers/plans/2026-07-01-photon-interface-contracts.md`.
//!
//! [`otlp_logs_to_records`] is the pure, unit-tested core: it flattens an OTLP
//! `ExportLogsServiceRequest` into Photon [`LogRecord`](photon_core::record::LogRecord)s.
//! [`IngestServer`] wraps it with two network front ends — a tonic `LogsService` (gRPC) and
//! an axum `POST /v1/logs` handler (HTTP, `application/x-protobuf`) — both of which check a
//! shared bearer token, map the request, build a `RecordBatch`, and append it to the WAL.

mod auth;
mod grpc;
mod grpc_metrics;
mod grpc_trace;
mod http;
mod mapping;
mod metrics_http;
mod metrics_mapping;
mod otlp_value;
mod promrw_http;
mod promrw_mapping;
mod promrw_proto;
mod trace_http;
mod trace_mapping;

pub use mapping::{otlp_logs_into_builder, otlp_logs_to_records};
pub use metrics_mapping::{otlp_metrics_into_builder, otlp_metrics_to_points};
pub use promrw_mapping::promrw_to_points;
pub use promrw_proto::{Label, Sample, TimeSeries, WriteRequest};
pub use trace_mapping::{otlp_traces_into_builder, otlp_traces_to_spans};

use axum::extract::DefaultBodyLimit;
use axum::Router;
use grpc::GrpcLogsService;
use grpc_metrics::GrpcMetricsService;
use grpc_trace::GrpcTraceService;
use http::HttpState;
use metrics_http::MetricsHttpState;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::LogsServiceServer;
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::MetricsServiceServer;
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::TraceServiceServer;
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_schema::MetricSchema;
use photon_core::schema::LogSchema;
use photon_core::span_schema::SpanSchema;
use photon_core::PhotonError;
use photon_wal::Wal;
use promrw_http::PromRwHttpState;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Semaphore;
use trace_http::TraceHttpState;

/// OTLP ingestion front end: gRPC (`LogsService` + `TraceService` + `MetricsService`) and HTTP
/// (`POST /v1/logs` + `POST /v1/traces` + `POST /v1/metrics`) receivers. Logs, traces, and
/// metrics share the bearer token but write to separate WALs with separate schemas.
pub struct IngestServer<W: Wal + Send + Sync + 'static> {
    wal: Arc<W>,
    spans_wal: Arc<W>,
    metrics_wal: Arc<W>,
    token: String,
    schema: LogSchema,
    span_schema: SpanSchema,
    metric_schema: MetricSchema,
    /// WS4 backpressure: max concurrently in-flight requests per signal (HTTP + gRPC combined,
    /// decode→build→append). Logs, traces, and metrics each get their own `max_in_flight`-sized
    /// semaphore, so a flood in one signal can't starve another and peak decoded memory is
    /// bounded per signal. From `[ingest].max_in_flight` (default 256).
    max_in_flight: usize,
    /// Max request body size in bytes for both front doors. HTTP enforces it on the
    /// **decompressed** stream (`DefaultBodyLimit` sitting inside the gzip decompression layer);
    /// gRPC mirrors it via `max_decoding_message_size` so the two agree. Also the snappy
    /// decompress cap for the Prometheus remote-write receiver. From `[ingest].max_body_bytes`
    /// (default ~16 MiB).
    max_body_bytes: usize,
    /// Per-signal cumulative ingest tallies, incremented after each successful WAL append.
    /// Shared with the usage sampler (`photon-server`) so `/api/usage/series` can report
    /// ingest rates without touching the WAL or the compactor.
    counters: Arc<IngestCounters>,
}

impl<W: Wal + Send + Sync + 'static> IngestServer<W> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wal: Arc<W>,
        spans_wal: Arc<W>,
        metrics_wal: Arc<W>,
        token: String,
        schema: LogSchema,
        span_schema: SpanSchema,
        metric_schema: MetricSchema,
        max_in_flight: usize,
        max_body_bytes: usize,
        counters: Arc<IngestCounters>,
    ) -> IngestServer<W> {
        IngestServer {
            wal,
            spans_wal,
            metrics_wal,
            token,
            schema,
            span_schema,
            metric_schema,
            max_in_flight,
            max_body_bytes,
            counters,
        }
    }

    /// Serve gRPC on `grpc_addr` and HTTP on `http_addr` concurrently. Resolves only when
    /// either server exits (normally that means the process is shutting down / one of them
    /// hit a fatal I/O error).
    pub async fn serve(
        self,
        grpc_addr: SocketAddr,
        http_addr: SocketAddr,
    ) -> Result<(), PhotonError> {
        // WS4 backpressure: one semaphore per signal, shared by that signal's HTTP and gRPC
        // front ends — each bounds how many of that signal's requests can be decoding/building/
        // appending at once, so a saturation burst waits for a permit instead of piling
        // decoded batches on the heap. Per-signal semaphores mean a flood in one signal can't
        // starve another.
        let in_flight = Arc::new(Semaphore::new(self.max_in_flight));
        let traces_in_flight = Arc::new(Semaphore::new(self.max_in_flight));
        let metrics_in_flight = Arc::new(Semaphore::new(self.max_in_flight));
        // Prometheus remote-write shares the metrics WAL but gets its own per-signal semaphore
        // (same sizing), matching the OTLP receivers.
        let promrw_in_flight = Arc::new(Semaphore::new(self.max_in_flight));

        let grpc_service = GrpcLogsService {
            wal: self.wal.clone(),
            token: self.token.clone(),
            schema: self.schema.clone(),
            in_flight: in_flight.clone(),
            counters: self.counters.clone(),
        };
        let grpc_trace_service = GrpcTraceService {
            wal: self.spans_wal.clone(),
            token: self.token.clone(),
            schema: self.span_schema.clone(),
            in_flight: traces_in_flight.clone(),
            counters: self.counters.clone(),
        };
        let grpc_metrics_service = GrpcMetricsService {
            wal: self.metrics_wal.clone(),
            token: self.token.clone(),
            schema: self.metric_schema.clone(),
            in_flight: metrics_in_flight.clone(),
            counters: self.counters.clone(),
        };
        // `accept_compressed(Gzip)` lets a stock OTel Collector (which gzips gRPC by default)
        // talk to us; `max_decoding_message_size(max_body_bytes)` makes the gRPC front door agree
        // with the HTTP `DefaultBodyLimit` instead of using tonic's separate 4 MiB default.
        use tonic::codec::CompressionEncoding;
        let grpc_future = tonic::transport::Server::builder()
            .add_service(
                LogsServiceServer::new(grpc_service)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(self.max_body_bytes),
            )
            .add_service(
                TraceServiceServer::new(grpc_trace_service)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(self.max_body_bytes),
            )
            .add_service(
                MetricsServiceServer::new(grpc_metrics_service)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(self.max_body_bytes),
            )
            .serve(grpc_addr);

        let http_state = Arc::new(HttpState {
            wal: self.wal.clone(),
            token: self.token.clone(),
            schema: self.schema.clone(),
            in_flight: in_flight.clone(),
            counters: self.counters.clone(),
        });
        let trace_state = Arc::new(TraceHttpState {
            wal: self.spans_wal.clone(),
            token: self.token.clone(),
            schema: self.span_schema.clone(),
            in_flight: traces_in_flight.clone(),
            counters: self.counters.clone(),
        });
        let metrics_state = Arc::new(MetricsHttpState {
            wal: self.metrics_wal.clone(),
            token: self.token.clone(),
            schema: self.metric_schema.clone(),
            in_flight: metrics_in_flight.clone(),
            counters: self.counters.clone(),
        });
        let promrw_state = Arc::new(PromRwHttpState {
            wal: self.metrics_wal.clone(),
            token: self.token.clone(),
            schema: self.metric_schema.clone(),
            in_flight: promrw_in_flight.clone(),
            max_body_bytes: self.max_body_bytes,
            counters: self.counters.clone(),
        });
        let app = build_http_router(
            http_state,
            trace_state,
            metrics_state,
            promrw_state,
            self.max_body_bytes,
        );
        let listener = tokio::net::TcpListener::bind(http_addr)
            .await
            .map_err(|e| PhotonError::Io(e.to_string()))?;
        let http_future = axum::serve(listener, app);

        tokio::try_join!(
            async {
                grpc_future
                    .await
                    .map_err(|e| PhotonError::Io(e.to_string()))
            },
            async {
                http_future
                    .await
                    .map_err(|e| PhotonError::Io(e.to_string()))
            },
        )?;

        Ok(())
    }
}

/// Build the merged ingest HTTP router with request-decompression + body-size limiting applied.
///
/// axum applies the LAST `.layer(...)` as the OUTERMOST, so a request flows through the
/// decompression layer first, then the body limit:
///
/// * `RequestDecompressionLayer` (added last → **outermost**) runs first and transparently
///   gunzips a `Content-Encoding: gzip` request body — a stock OTel Collector `otlphttp`
///   exporter gzips by default. Non-gzip bodies pass through untouched.
/// * `DefaultBodyLimit::max(max_body_bytes)` (added first → **inner**) sets the limit that the
///   `Bytes` extractor enforces: the extractor reads the request body — by then already the
///   **decompressed** stream — through `http_body_util::Limited`, so the cap counts DECOMPRESSED
///   bytes and the stream is aborted (413) the moment it exceeds the limit. A small gzip bomb is
///   rejected without buffering gigabytes.
///
/// This is the spec-mandated ordering (decompression outermost, limit inner). Note the 413
/// behaviour is actually robust to the *order* of these two `.layer` calls here, because
/// `DefaultBodyLimit` doesn't wrap the body at its own position — it only stores the limit, and
/// the innermost `Bytes` extractor always sees the fully decompressed body (the `gzip_*` tests
/// confirm 413 on decompressed-oversize). The ordering still matters defensively: if the limit
/// were ever swapped for an *active* body-wrapping layer (e.g. `RequestBodyLimitLayer`, which
/// wraps at its own position), decompression-outermost is the only order that counts decompressed
/// bytes rather than the compressed input.
fn build_http_router<W: Wal + Send + Sync + 'static>(
    http_state: Arc<HttpState<W>>,
    trace_state: Arc<TraceHttpState<W>>,
    metrics_state: Arc<MetricsHttpState<W>>,
    promrw_state: Arc<PromRwHttpState<W>>,
    max_body_bytes: usize,
) -> Router {
    http::router(http_state)
        .merge(trace_http::router(trace_state))
        .merge(metrics_http::router(metrics_state))
        .merge(promrw_http::router(promrw_state))
        // Inner: bound the DECOMPRESSED body (see the doc comment for why this is added first).
        .layer(DefaultBodyLimit::max(max_body_bytes))
        // Outer: decompress the request body first, so the limit above counts decompressed bytes.
        .layer(tower_http::decompression::RequestDecompressionLayer::new())
}

/// The load-bearing interop test (fix-notes item 6): the merged ingest router, driven through its
/// real middleware stack, must (1) transparently gunzip a gzipped OTLP body → 2xx (stock OTel
/// Collector interop) and (2) reject a body whose **decompressed** size exceeds `max_body_bytes`
/// with 413 (proving the body limit bounds the *decompressed* stream, not the small compressed
/// input). See `build_http_router`'s doc comment for why the 413 holds regardless of the two
/// layers' relative order under axum's `DefaultBodyLimit` mechanism.
#[cfg(test)]
mod gzip_interop_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::logs::v1::{
        LogRecord as OtlpLogRecord, ResourceLogs, ScopeLogs,
    };
    use photon_core::ingest_counters::IngestCounters;
    use photon_core::segment::SegmentId;
    use std::io::Write as _;
    use tower::ServiceExt as _; // for `oneshot`

    /// Always-succeeding WAL so a decoded-and-appended request returns 200.
    struct OkWal;
    #[allow(clippy::manual_async_fn)]
    impl Wal for OkWal {
        fn append(
            &self,
            _batch: arrow::record_batch::RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { Ok(()) }
        }
        fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { Ok(()) }
        }
        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            Ok(vec![])
        }
        fn read_segment(
            &self,
            _id: SegmentId,
        ) -> impl std::future::Future<
            Output = Result<Vec<arrow::record_batch::RecordBatch>, PhotonError>,
        > + Send {
            async move { Ok(vec![]) }
        }
        fn remove_segment(&self, _id: SegmentId) -> Result<(), PhotonError> {
            Ok(())
        }
    }

    /// Build the real merged router (all four sub-routers + decompression + body-limit layers)
    /// over an `OkWal`, with the given `max_body_bytes`.
    fn router(max_body_bytes: usize) -> Router {
        let wal = Arc::new(OkWal);
        let counters = Arc::new(IngestCounters::new());
        let in_flight = Arc::new(Semaphore::new(8));
        let promoted = ["service.name".to_string()];
        build_http_router(
            Arc::new(HttpState {
                wal: wal.clone(),
                token: "t".to_string(),
                schema: LogSchema::new(&promoted),
                in_flight: in_flight.clone(),
                counters: counters.clone(),
            }),
            Arc::new(TraceHttpState {
                wal: wal.clone(),
                token: "t".to_string(),
                schema: SpanSchema::new(&promoted),
                in_flight: in_flight.clone(),
                counters: counters.clone(),
            }),
            Arc::new(MetricsHttpState {
                wal: wal.clone(),
                token: "t".to_string(),
                schema: MetricSchema::new(&promoted),
                in_flight: in_flight.clone(),
                counters: counters.clone(),
            }),
            Arc::new(PromRwHttpState {
                wal: wal.clone(),
                token: "t".to_string(),
                schema: MetricSchema::new(&promoted),
                in_flight: in_flight.clone(),
                max_body_bytes,
                counters: counters.clone(),
            }),
            max_body_bytes,
        )
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    /// A valid OTLP logs request carrying one bare log record, protobuf-encoded.
    fn valid_logs_body() -> Vec<u8> {
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![OtlpLogRecord {
                        time_unix_nano: 1,
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let mut buf = Vec::new();
        <ExportLogsServiceRequest as prost::Message>::encode(&req, &mut buf).unwrap();
        buf
    }

    /// (1) A *gzipped* valid OTLP body with `Content-Encoding: gzip` is transparently
    /// decompressed, decoded, and appended → 200. Proves stock-Collector interop.
    #[tokio::test]
    async fn gzipped_otlp_logs_body_is_decompressed_and_accepted() {
        let app = router(16 * 1024 * 1024);
        let body = gzip(&valid_logs_body());
        let req = Request::builder()
            .method("POST")
            .uri("/v1/logs")
            .header(header::AUTHORIZATION, "Bearer t")
            .header(header::CONTENT_ENCODING, "gzip")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "gzipped OTLP body must be transparently decompressed and accepted"
        );
    }

    /// (2) A gzipped body whose **decompressed** size exceeds `max_body_bytes` → 413. Because the
    /// body is small compressed but large decompressed, a 413 here proves the limit counts
    /// DECOMPRESSED bytes — i.e. decompression is outermost and the limit is inner. `Limited`
    /// aborts the stream at the cap, so no gigabytes are buffered (the gzip-bomb defense).
    #[tokio::test]
    async fn gzipped_body_over_decompressed_limit_is_413() {
        let max_body_bytes = 1024;
        let app = router(max_body_bytes);
        // 8 KiB of zeros gzips to a few dozen bytes but decompresses to 8192 > 1024.
        let raw = vec![0u8; 8192];
        let compressed = gzip(&raw);
        assert!(
            compressed.len() < max_body_bytes,
            "compressed input must be under the limit so only the DECOMPRESSED size can trip it"
        );
        let req = Request::builder()
            .method("POST")
            .uri("/v1/logs")
            .header(header::AUTHORIZATION, "Bearer t")
            .header(header::CONTENT_ENCODING, "gzip")
            .body(Body::from(compressed))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "the body limit must bound the DECOMPRESSED stream (413), not the compressed input"
        );
    }
}
