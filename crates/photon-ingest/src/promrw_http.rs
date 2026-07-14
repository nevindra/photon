//! axum `POST /api/v1/write` receiver: token check → snappy-decompress → protobuf decode →
//! `WriteRequest`→`MetricPoint` mapping → metrics-WAL append. Prometheus remote-write 1.0.
//! Snappy/decode/auth failures are rejected before the WAL is touched. Writes the *same*
//! metrics WAL as the OTLP `/v1/metrics` receiver.

use crate::auth::check_bearer_token;
use crate::promrw_mapping::promrw_to_points;
use crate::promrw_proto::WriteRequest;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_record::MetricBatchBuilder;
use photon_core::metric_schema::MetricSchema;
use photon_core::PhotonError;
use photon_wal::Wal;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub(crate) struct PromRwHttpState<W: Wal + Send + Sync + 'static> {
    pub(crate) wal: Arc<W>,
    pub(crate) token: String,
    pub(crate) schema: MetricSchema,
    /// Its own per-signal semaphore (sized from `[ingest].max_in_flight`), matching the OTLP
    /// receivers — bounds concurrently in-flight remote-write requests (decompress→decode→build→
    /// append) so a burst waits for a permit instead of piling decoded batches on the heap.
    pub(crate) in_flight: Arc<Semaphore>,
    /// Cumulative ingest tallies (the `metrics` signal), incremented after a successful append.
    pub(crate) counters: Arc<IngestCounters>,
}

/// Snappy-decompress then protobuf-decode a remote-write body. Pure — no I/O — so the
/// "malformed payload rejected before the WAL" behaviour is unit-testable without a live server.
pub(crate) fn decode_write_request(body: &[u8]) -> Result<WriteRequest, PhotonError> {
    let mut decoder = snap::raw::Decoder::new();
    let decompressed = decoder
        .decompress_vec(body)
        .map_err(|e| PhotonError::Config(format!("invalid snappy remote-write payload: {e}")))?;
    <WriteRequest as prost::Message>::decode(decompressed.as_slice())
        .map_err(|e| PhotonError::Config(format!("invalid remote-write protobuf payload: {e}")))
}

pub(crate) fn router<W: Wal + Send + Sync + 'static>(state: Arc<PromRwHttpState<W>>) -> Router {
    Router::new()
        .route("/api/v1/write", post(ingest_promrw::<W>))
        .with_state(state)
}

async fn ingest_promrw<W: Wal + Send + Sync + 'static>(
    State(state): State<Arc<PromRwHttpState<W>>>,
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

    let req = match decode_write_request(&body) {
        Ok(req) => req,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let points = promrw_to_points(req);
    let mut builder = MetricBatchBuilder::with_capacity(&state.schema, points.len());
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
    use crate::promrw_proto::{Label, Sample, TimeSeries};
    use arrow::array::{Array, RecordBatch, StringArray};
    use photon_core::metric_schema::METRIC_NAME;
    use photon_core::segment::SegmentId;
    use prost::Message as _;

    /// A `Wal` that records appended batches so the handler's map→append can be asserted.
    #[derive(Default)]
    struct CapturingWal {
        batches: std::sync::Mutex<Vec<RecordBatch>>,
    }
    #[allow(clippy::manual_async_fn)]
    impl Wal for CapturingWal {
        fn append(
            &self,
            batch: RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            self.batches.lock().unwrap().push(batch);
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
        ) -> impl std::future::Future<Output = Result<Vec<RecordBatch>, PhotonError>> + Send
        {
            async move { Ok(vec![]) }
        }
        fn remove_segment(&self, _id: SegmentId) -> Result<(), PhotonError> {
            Ok(())
        }
    }

    fn snappy(req: &WriteRequest) -> Vec<u8> {
        let proto = req.encode_to_vec();
        let mut encoder = snap::raw::Encoder::new();
        encoder.compress_vec(&proto).unwrap()
    }

    fn state(wal: Arc<CapturingWal>) -> Arc<PromRwHttpState<CapturingWal>> {
        Arc::new(PromRwHttpState {
            wal,
            token: "t".to_string(),
            schema: MetricSchema::new(&["service.name".to_string()]),
            in_flight: Arc::new(Semaphore::new(4)),
            counters: Arc::new(IngestCounters::new()),
        })
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        h
    }

    #[tokio::test]
    async fn valid_request_maps_and_appends() {
        let wal = Arc::new(CapturingWal::default());
        let req = WriteRequest {
            timeseries: vec![TimeSeries {
                labels: vec![
                    Label {
                        name: "__name__".into(),
                        value: "http_requests_total".into(),
                    },
                    Label {
                        name: "job".into(),
                        value: "api".into(),
                    },
                ],
                samples: vec![Sample {
                    value: 7.0,
                    timestamp: 1_700_000_000_000,
                }],
            }],
        };
        let resp = ingest_promrw(
            State(state(wal.clone())),
            bearer("t"),
            Bytes::from(snappy(&req)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let batches = wal.batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.num_rows(), 1);
        let names = batch
            .column_by_name(METRIC_NAME)
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(names.value(0), "http_requests_total");
        let svc = batch
            .column_by_name("service.name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(svc.value(0), "api");
    }

    #[tokio::test]
    async fn missing_token_is_unauthorized_and_appends_nothing() {
        let wal = Arc::new(CapturingWal::default());
        let req = WriteRequest { timeseries: vec![] };
        let resp = ingest_promrw(
            State(state(wal.clone())),
            HeaderMap::new(),
            Bytes::from(snappy(&req)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(wal.batches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn malformed_body_is_bad_request_and_appends_nothing() {
        let wal = Arc::new(CapturingWal::default());
        // 0xFF.. is neither valid snappy-block nor protobuf.
        let resp = ingest_promrw(
            State(state(wal.clone())),
            bearer("t"),
            Bytes::from(vec![0xFFu8; 8]),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(wal.batches.lock().unwrap().is_empty());
    }

    #[test]
    fn decode_rejects_non_snappy_bytes() {
        let result = decode_write_request(&[0xFFu8; 8]);
        assert!(matches!(result, Err(PhotonError::Config(_))));
    }

    #[test]
    fn promrw_http_state_bounds_in_flight_permits() {
        let s = state(Arc::new(CapturingWal::default()));
        let s = PromRwHttpState {
            wal: s.wal.clone(),
            token: s.token.clone(),
            schema: s.schema.clone(),
            in_flight: Arc::new(Semaphore::new(1)),
            counters: s.counters.clone(),
        };
        let _first = s
            .in_flight
            .clone()
            .try_acquire_owned()
            .expect("first permit available with max_in_flight = 1");
        assert!(
            s.in_flight.clone().try_acquire_owned().is_err(),
            "second concurrent permit refused while the first is held"
        );
    }
}
