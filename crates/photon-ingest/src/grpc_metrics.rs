//! tonic `MetricsService`: token check → OTLP→`MetricPoint` mapping → metrics-WAL append.

use crate::auth::{resolve_bearer, stamp_tenant_metrics, Auth};
use crate::metrics_mapping::{estimate_rows, otlp_metrics_into_builder};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::MetricsService, ExportMetricsServiceRequest,
    ExportMetricsServiceResponse,
};
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_record::MetricBatchBuilder;
use photon_core::metric_schema::MetricSchema;
use photon_core::TenantTokenMap;
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
    /// Federation: bearer token -> tenant name, consulted when the local token doesn't
    /// match. Empty on a non-central node, so tenant auth simply never matches.
    pub(crate) tenant_tokens: TenantTokenMap,
    /// Federation `full` mode: the accepted request is re-encoded and offered here for
    /// best-effort forwarding to central (gRPC never sees the raw wire bytes).
    pub(crate) federation_tee: Option<crate::FederationTee>,
}

#[tonic::async_trait]
impl<W: Wal + Send + Sync + 'static> MetricsService for GrpcMetricsService<W> {
    async fn export(
        &self,
        request: tonic::Request<ExportMetricsServiceRequest>,
    ) -> Result<tonic::Response<ExportMetricsServiceResponse>, tonic::Status> {
        // Cheap token check first so an unauthenticated flood is rejected before it ever
        // competes for an in-flight permit; the permit exists to bound expensive work
        // (decode→build→append), not free rejections.
        let auth_header = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        let auth = resolve_bearer(auth_header, &self.token, &self.tenant_tokens);
        if auth == Auth::Denied {
            return Err(tonic::Status::unauthenticated(
                "missing or invalid bearer token",
            ));
        }

        let _permit = self.in_flight.clone().acquire_owned().await.map_err(|e| {
            tonic::Status::resource_exhausted(format!("ingest temporarily overloaded: {e}"))
        })?;

        let mut req = request.into_inner();
        if let Auth::Tenant(tenant) = &auth {
            stamp_tenant_metrics(&mut req, tenant);
        }
        // Full-mode federation: prost re-encode (gRPC decoded the body for us). try_send-only,
        // so a stalled forwarder can never delay the ack.
        if let Some(tee) = &self.federation_tee {
            tee.offer(
                crate::TeeSignal::Metrics,
                prost::Message::encode_to_vec(&req).into(),
            );
        }
        let mut builder = MetricBatchBuilder::with_capacity(&self.schema, estimate_rows(&req));
        otlp_metrics_into_builder(req, &mut builder);
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
