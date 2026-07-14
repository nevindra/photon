//! axum `POST /v1/metrics` receiver: token check → protobuf decode → OTLP→`MetricPoint`
//! mapping → metrics-WAL append. Decode errors (and auth failures) are rejected before the WAL
//! is touched.

use crate::auth::check_bearer_token;
use crate::otlp_metrics_to_points;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_record::MetricBatchBuilder;
use photon_core::metric_schema::MetricSchema;
use photon_core::PhotonError;
use photon_wal::Wal;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub(crate) struct MetricsHttpState<W: Wal + Send + Sync + 'static> {
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

/// Decode a raw protobuf body into an `ExportMetricsServiceRequest`. Pure — no I/O — so the
/// "malformed payload is rejected before the WAL is touched" behaviour is unit-testable
/// without a live server.
pub(crate) fn decode_metrics_request(
    body: &[u8],
) -> Result<ExportMetricsServiceRequest, PhotonError> {
    <ExportMetricsServiceRequest as prost::Message>::decode(body)
        .map_err(|e| PhotonError::Config(format!("invalid OTLP protobuf payload: {e}")))
}

pub(crate) fn router<W: Wal + Send + Sync + 'static>(state: Arc<MetricsHttpState<W>>) -> Router {
    Router::new()
        .route("/v1/metrics", post(ingest_metrics::<W>))
        .with_state(state)
}

async fn ingest_metrics<W: Wal + Send + Sync + 'static>(
    State(state): State<Arc<MetricsHttpState<W>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let _permit = match state.in_flight.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("ingest temporarily overloaded: {e}"),
            )
                .into_response()
        }
    };

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if !check_bearer_token(auth_header, &state.token) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    }

    let req = match decode_metrics_request(&body) {
        Ok(req) => req,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let points = otlp_metrics_to_points(req);
    let mut builder = MetricBatchBuilder::new(&state.schema);
    for point in &points {
        builder.append(point);
    }
    let batch = match builder.finish() {
        Ok(batch) => batch,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let rows = batch.num_rows() as u64;
    let bytes = batch.get_array_memory_size() as u64;
    if let Err(e) = state.wal.append(batch).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    state.counters.metrics.add(rows, bytes);

    StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rejects_malformed_bytes() {
        // 0xFF has its continuation bit set with no following bytes -> invalid varint tag.
        let malformed = [0xFFu8; 8];
        let result = decode_metrics_request(&malformed);
        assert!(result.is_err());
        match result {
            Err(PhotonError::Config(msg)) => assert!(msg.contains("invalid OTLP")),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn decode_accepts_empty_request() {
        let result = decode_metrics_request(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().resource_metrics.is_empty());
    }

    #[test]
    fn metrics_http_state_bounds_in_flight_permits() {
        use photon_core::segment::SegmentId;
        use tokio::sync::Semaphore;

        struct FakeWal;
        #[allow(clippy::manual_async_fn)]
        impl Wal for FakeWal {
            fn append(
                &self,
                _batch: arrow::record_batch::RecordBatch,
            ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
                async move { unimplemented!() }
            }
            fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
                async move { unimplemented!() }
            }
            fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
                unimplemented!()
            }
            fn read_segment(
                &self,
                _id: SegmentId,
            ) -> impl std::future::Future<
                Output = Result<Vec<arrow::record_batch::RecordBatch>, PhotonError>,
            > + Send {
                async move { unimplemented!() }
            }
            fn remove_segment(&self, _id: SegmentId) -> Result<(), PhotonError> {
                unimplemented!()
            }
        }

        let state = MetricsHttpState {
            wal: Arc::new(FakeWal),
            token: "t".to_string(),
            schema: MetricSchema::new(&["service.name".to_string()]),
            in_flight: Arc::new(Semaphore::new(1)),
            counters: Arc::new(photon_core::ingest_counters::IngestCounters::new()),
        };

        let _first = state
            .in_flight
            .clone()
            .try_acquire_owned()
            .expect("first permit should be available with max_in_flight = 1");
        assert!(
            state.in_flight.clone().try_acquire_owned().is_err(),
            "second concurrent permit should be refused while the first is held"
        );
    }
}
