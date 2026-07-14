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
pub use metrics_mapping::otlp_metrics_to_points;
pub use promrw_mapping::promrw_to_points;
pub use promrw_proto::{Label, Sample, TimeSeries, WriteRequest};
pub use trace_mapping::otlp_traces_to_spans;

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
        let grpc_future = tonic::transport::Server::builder()
            .add_service(LogsServiceServer::new(grpc_service))
            .add_service(TraceServiceServer::new(grpc_trace_service))
            .add_service(MetricsServiceServer::new(grpc_metrics_service))
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
            counters: self.counters.clone(),
        });
        let app = http::router(http_state)
            .merge(trace_http::router(trace_state))
            .merge(metrics_http::router(metrics_state))
            .merge(promrw_http::router(promrw_state));
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
