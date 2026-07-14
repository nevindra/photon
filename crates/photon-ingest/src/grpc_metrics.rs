//! tonic `MetricsService`: token check → OTLP→`MetricPoint` mapping → metrics-WAL append.

use crate::auth::check_bearer_token;
use crate::otlp_metrics_to_points;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::MetricsService, ExportMetricsServiceRequest,
    ExportMetricsServiceResponse,
};
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_record::MetricBatchBuilder;
use photon_core::metric_schema::MetricSchema;
use photon_wal::Wal;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub(crate) struct GrpcMetricsService<W: Wal + Send + Sync + 'static> {
    pub(crate) wal: Arc<W>,
    pub(crate) token: String,
    pub(crate) schema: MetricSchema,
    /// WS4 backpressure: bounds concurrently in-flight metrics requests (decode→build→append) so
    /// a saturation burst waits for a permit instead of piling decoded batches on the heap. Its
    /// own per-signal semaphore, sized from `[ingest].max_in_flight`.
    pub(crate) in_flight: Arc<Semaphore>,
    /// Cumulative ingest tallies, incremented after a successful WAL append.
    pub(crate) counters: Arc<IngestCounters>,
}

#[tonic::async_trait]
impl<W: Wal + Send + Sync + 'static> MetricsService for GrpcMetricsService<W> {
    async fn export(
        &self,
        request: tonic::Request<ExportMetricsServiceRequest>,
    ) -> Result<tonic::Response<ExportMetricsServiceResponse>, tonic::Status> {
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

        let points = otlp_metrics_to_points(request.into_inner());
        let mut builder = MetricBatchBuilder::new(&self.schema);
        for point in &points {
            builder.append(point);
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
        self.counters.metrics.add(rows, bytes);

        Ok(tonic::Response::new(ExportMetricsServiceResponse::default()))
    }
}
