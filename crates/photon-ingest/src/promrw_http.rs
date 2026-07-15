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
    /// Snappy decompress cap (bytes), from `[ingest].max_body_bytes` — the same limit the OTLP
    /// HTTP front door enforces on decompressed bodies. A snappy frame header claims a
    /// decompressed length that `decompress_vec` allocates up-front, so a tiny remote-write body
    /// could otherwise demand a giant buffer; we reject (413) when the claimed length exceeds it.
    pub(crate) max_body_bytes: usize,
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

/// The size a snappy body *claims* it will decompress to (read cheaply from the frame header),
/// before `decode_write_request` allocates that much. `None` if the header is unreadable (a
/// malformed body — `decode_write_request` then produces the 400). Pure and unit-testable.
pub(crate) fn claimed_decompressed_len(body: &[u8]) -> Option<usize> {
    snap::raw::decompress_len(body).ok()
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
    // Cheap token check first so an unauthenticated flood is rejected before it ever
    // competes for an in-flight permit; the permit exists to bound expensive work
    // (decompress→decode→build→append), not free rejections.
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if !check_bearer_token(auth_header, &state.token) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    }

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

    // Cap the *claimed* decompressed size before `decode_write_request` allocates it: a snappy
    // frame header can claim gigabytes from a tiny body. Reject over-cap with 413, matching the
    // OTLP HTTP front door's `DefaultBodyLimit` semantics (an exporter distinguishes 413 from the
    // 400 we return for a genuinely malformed/undecodable body).
    if let Some(len) = claimed_decompressed_len(&body) {
        if len > state.max_body_bytes {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "remote-write body decompresses to {len} bytes, over the \
                     {} byte limit",
                    state.max_body_bytes
                ),
            )
                .into_response();
        }
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
            max_body_bytes: 16 * 1024 * 1024,
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
    fn claimed_decompressed_len_reads_the_snappy_header() {
        // Snappy of 4 KiB of zeros compresses tiny but claims 4096 decompressed bytes.
        let mut encoder = snap::raw::Encoder::new();
        let compressed = encoder.compress_vec(&vec![0u8; 4096]).unwrap();
        assert!(
            compressed.len() < 4096,
            "should compress well below its claim"
        );
        assert_eq!(claimed_decompressed_len(&compressed), Some(4096));
        // A non-snappy body has no readable claim → None (the 400 path handles it).
        assert_eq!(claimed_decompressed_len(&[0xFFu8; 8]), None);
    }

    /// A small snappy body that *claims* to decompress past `max_body_bytes` is rejected with 413
    /// BEFORE `decode_write_request` allocates that buffer — the snappy analogue of the OTLP HTTP
    /// front door's decompressed-stream body limit.
    #[tokio::test]
    async fn oversize_snappy_body_is_payload_too_large_and_appends_nothing() {
        let wal = Arc::new(CapturingWal::default());
        let state = Arc::new(PromRwHttpState {
            wal: wal.clone(),
            token: "t".to_string(),
            schema: MetricSchema::new(&["service.name".to_string()]),
            in_flight: Arc::new(Semaphore::new(4)),
            max_body_bytes: 64, // tiny cap so we allocate nothing large
            counters: Arc::new(IngestCounters::new()),
        });
        let mut encoder = snap::raw::Encoder::new();
        let compressed = encoder.compress_vec(&vec![0u8; 4096]).unwrap(); // claims 4096 > 64
        let resp = ingest_promrw(State(state), bearer("t"), Bytes::from(compressed)).await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(wal.batches.lock().unwrap().is_empty());
    }

    #[test]
    fn promrw_http_state_bounds_in_flight_permits() {
        let s = state(Arc::new(CapturingWal::default()));
        let s = PromRwHttpState {
            wal: s.wal.clone(),
            token: s.token.clone(),
            schema: s.schema.clone(),
            in_flight: Arc::new(Semaphore::new(1)),
            max_body_bytes: s.max_body_bytes,
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

    /// Auth must be checked BEFORE the in-flight permit is acquired: hold the crate's only
    /// permit for the whole test, then send an invalid-token request. If the handler acquired
    /// the permit before checking the token, `ingest_promrw` would block forever waiting on a
    /// permit that never frees; the bounded `timeout` turns that hang into a clear test failure
    /// instead of a wedged test run.
    #[tokio::test]
    async fn invalid_token_is_rejected_without_waiting_for_a_permit() {
        let wal = Arc::new(CapturingWal::default());
        let in_flight = Arc::new(Semaphore::new(1));
        let state = Arc::new(PromRwHttpState {
            wal: wal.clone(),
            token: "t".to_string(),
            schema: MetricSchema::new(&["service.name".to_string()]),
            in_flight: in_flight.clone(),
            max_body_bytes: 16 * 1024 * 1024,
            counters: Arc::new(IngestCounters::new()),
        });
        let _held = in_flight
            .acquire_owned()
            .await
            .expect("the sole permit should be available before the test holds it");

        let req = WriteRequest { timeseries: vec![] };
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ingest_promrw(State(state), bearer("wrong"), Bytes::from(snappy(&req))),
        )
        .await
        .expect("token check must reject before waiting on the in-flight permit");

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(wal.batches.lock().unwrap().is_empty());
    }
}
