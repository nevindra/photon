//! tonic `LogsService` implementation: token check → OTLP→`LogRecord` mapping → WAL append.

use crate::auth::check_bearer_token;
use crate::mapping::otlp_logs_into_builder;
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_server::LogsService, ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use photon_core::ingest_counters::IngestCounters;
use photon_core::record::RecordBatchBuilder;
use photon_core::schema::LogSchema;
use photon_wal::Wal;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub(crate) struct GrpcLogsService<W: Wal + Send + Sync + 'static> {
    pub(crate) wal: Arc<W>,
    pub(crate) token: String,
    pub(crate) schema: LogSchema,
    /// WS4 backpressure: bounds concurrently in-flight logs requests (decode→build→append)
    /// so a saturation burst waits for a permit instead of piling decoded batches on the
    /// heap. Sized from `[ingest].max_in_flight` (default 256); see `IngestConfig`.
    pub(crate) in_flight: Arc<Semaphore>,
    /// Cumulative ingest tallies, incremented after a successful WAL append.
    pub(crate) counters: Arc<IngestCounters>,
}

#[tonic::async_trait]
impl<W: Wal + Send + Sync + 'static> LogsService for GrpcLogsService<W> {
    async fn export(
        &self,
        request: tonic::Request<ExportLogsServiceRequest>,
    ) -> Result<tonic::Response<ExportLogsServiceResponse>, tonic::Status> {
        // Cheap token check first so an unauthenticated flood is rejected before it ever
        // competes for an in-flight permit; the permit exists to bound expensive work
        // (decode→build→append), not free rejections.
        let auth_header = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if !check_bearer_token(auth_header, &self.token) {
            return Err(tonic::Status::unauthenticated(
                "missing or invalid bearer token",
            ));
        }

        // WS4 backpressure: acquire an in-flight permit before doing any decode/build/append
        // work; excess requests wait here rather than piling decoded batches on the heap.
        // Held until the handler returns (RAII release on drop). The semaphore is never
        // closed, so `Err` is not expected in practice, but we still fail gracefully instead
        // of unwrapping.
        let _permit = self.in_flight.clone().acquire_owned().await.map_err(|e| {
            tonic::Status::resource_exhausted(format!("ingest temporarily overloaded: {e}"))
        })?;

        let req = request.into_inner();
        let mut builder = RecordBatchBuilder::with_capacity(&self.schema, estimate_rows(&req));
        otlp_logs_into_builder(req, &mut builder);
        let batch = builder
            .finish()
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let rows = batch.num_rows() as u64;
        let bytes = batch.get_array_memory_size() as u64;
        self.wal
            .append(batch)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
        self.counters.logs.add(rows, bytes);

        Ok(tonic::Response::new(ExportLogsServiceResponse::default()))
    }
}

/// Total log-record count across every resource/scope group, used to pre-size the
/// `RecordBatchBuilder` so its column builders don't pay for geometric reallocation.
/// Kept independent of `http::estimate_rows` (same shape, different file) so the two
/// transport modules don't cross-import.
fn estimate_rows(req: &ExportLogsServiceRequest) -> usize {
    req.resource_logs
        .iter()
        .flat_map(|rl| &rl.scope_logs)
        .map(|sl| sl.log_records.len())
        .sum()
}
