//! tonic `TraceService`: token check → OTLP→`SpanRecord` mapping → spans-WAL append.

use crate::auth::check_bearer_token;
use crate::trace_mapping::otlp_traces_to_spans;
use opentelemetry_proto::tonic::collector::trace::v1::{
    trace_service_server::TraceService, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use photon_core::ingest_counters::IngestCounters;
use photon_core::span_record::SpanBatchBuilder;
use photon_core::span_schema::SpanSchema;
use photon_wal::Wal;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub(crate) struct GrpcTraceService<W: Wal + Send + Sync + 'static> {
    pub(crate) wal: Arc<W>,
    pub(crate) token: String,
    pub(crate) schema: SpanSchema,
    /// WS4 backpressure: bounds concurrently in-flight trace requests (decode→build→append) so a
    /// saturation burst waits for a permit instead of piling decoded batches on the heap. Its own
    /// per-signal semaphore, sized from `[ingest].max_in_flight`.
    pub(crate) in_flight: Arc<Semaphore>,
    /// Cumulative ingest tallies, incremented after a successful WAL append.
    pub(crate) counters: Arc<IngestCounters>,
}

#[tonic::async_trait]
impl<W: Wal + Send + Sync + 'static> TraceService for GrpcTraceService<W> {
    async fn export(
        &self,
        request: tonic::Request<ExportTraceServiceRequest>,
    ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
        let _permit = self.in_flight.clone().acquire_owned().await.map_err(|e| {
            tonic::Status::resource_exhausted(format!("ingest temporarily overloaded: {e}"))
        })?;

        let auth_header = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if !check_bearer_token(auth_header, &self.token) {
            return Err(tonic::Status::unauthenticated(
                "missing or invalid bearer token",
            ));
        }

        let spans = otlp_traces_to_spans(request.into_inner());
        let mut builder = SpanBatchBuilder::new(&self.schema);
        for span in &spans {
            builder.append(span);
        }
        let batch = builder
            .finish()
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let rows = batch.num_rows() as u64;
        let bytes = batch.get_array_memory_size() as u64;
        self.wal
            .append(batch)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
        self.counters.traces.add(rows, bytes);

        Ok(tonic::Response::new(ExportTraceServiceResponse::default()))
    }
}
