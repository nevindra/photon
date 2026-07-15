//! axum `POST /v1/logs` receiver: token check → protobuf decode → OTLP→`LogRecord` mapping
//! → WAL append. Decode errors (and auth failures) are rejected before the WAL is touched.

use crate::auth::check_bearer_token;
use crate::mapping::otlp_logs_into_builder;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use photon_core::ingest_counters::IngestCounters;
use photon_core::record::RecordBatchBuilder;
use photon_core::schema::LogSchema;
use photon_core::PhotonError;
use photon_wal::Wal;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub(crate) struct HttpState<W: Wal + Send + Sync + 'static> {
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

/// Decode a raw protobuf body into an `ExportLogsServiceRequest`. Pure — no I/O — so the
/// "malformed payload is rejected before the WAL is touched" behaviour is unit-testable
/// without a live server.
pub(crate) fn decode_export_request(body: &[u8]) -> Result<ExportLogsServiceRequest, PhotonError> {
    <ExportLogsServiceRequest as prost::Message>::decode(body)
        .map_err(|e| PhotonError::Config(format!("invalid OTLP protobuf payload: {e}")))
}

pub(crate) fn router<W: Wal + Send + Sync + 'static>(state: Arc<HttpState<W>>) -> Router {
    Router::new()
        .route("/v1/logs", post(ingest_logs::<W>))
        .with_state(state)
}

async fn ingest_logs<W: Wal + Send + Sync + 'static>(
    State(state): State<Arc<HttpState<W>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Cheap token check first so an unauthenticated flood is rejected before it ever
    // competes for an in-flight permit; the permit exists to bound expensive work
    // (decode→build→append), not free rejections.
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if !check_bearer_token(auth_header, &state.token) {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    }

    // WS4 backpressure: acquire an in-flight permit before doing any decode/build/append
    // work; excess requests wait here rather than piling decoded batches on the heap. Held
    // until the handler returns (RAII release on drop). The semaphore is never closed, so
    // `Err` is not expected in practice, but we still fail gracefully instead of unwrapping.
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

    let req = match decode_export_request(&body) {
        Ok(req) => req,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let mut builder = RecordBatchBuilder::with_capacity(&state.schema, estimate_rows(&req));
    otlp_logs_into_builder(req, &mut builder);
    let batch = match builder.finish() {
        Ok(batch) => batch,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let rows = batch.num_rows() as u64;
    let bytes = batch.get_array_memory_size() as u64;
    if let Err(e) = state.wal.append(batch).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    state.counters.logs.add(rows, bytes);

    StatusCode::OK.into_response()
}

/// Total log-record count across every resource/scope group, used to pre-size the
/// `RecordBatchBuilder` so its column builders don't pay for geometric reallocation.
fn estimate_rows(req: &ExportLogsServiceRequest) -> usize {
    req.resource_logs
        .iter()
        .flat_map(|rl| &rl.scope_logs)
        .map(|sl| sl.log_records.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rejects_malformed_bytes() {
        // 0xFF has its continuation bit set with no following bytes -> invalid varint tag.
        let malformed = [0xFFu8; 8];
        let result = decode_export_request(&malformed);
        assert!(result.is_err());
        match result {
            Err(PhotonError::Config(msg)) => assert!(msg.contains("invalid OTLP protobuf")),
            other => panic!("expected PhotonError::Config, got {other:?}"),
        }
    }

    #[test]
    fn decode_accepts_a_valid_empty_request() {
        // An ExportLogsServiceRequest with no resource_logs encodes to zero bytes.
        let result = decode_export_request(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().resource_logs.is_empty());
    }

    /// WS4 backpressure: `HttpState` carries an `Arc<Semaphore>` sized from
    /// `[ingest].max_in_flight`. With `max_in_flight = 1`, a second concurrent permit must be
    /// refused while the first is held — this is what stops a saturation burst from piling
    /// decoded batches on the heap past the configured bound (B1: unbounded in-flight OOM-killed
    /// the server under conc=128 saturate).
    #[test]
    fn http_state_bounds_in_flight_permits() {
        use photon_core::segment::SegmentId;
        use tokio::sync::Semaphore;

        struct FakeWal;

        // `impl Future + Send` mirrors the trait's signature exactly (see the identical
        // pattern in photon-compact's FakeWal); none of these are exercised by this test.
        #[allow(clippy::manual_async_fn)]
        impl Wal for FakeWal {
            fn append(
                &self,
                _batch: arrow::record_batch::RecordBatch,
            ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
                async move { unimplemented!("FakeWal::append is not exercised by this test") }
            }
            fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
                async move { unimplemented!("FakeWal::sync is not exercised by this test") }
            }
            fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
                unimplemented!("FakeWal::list_closed_segments is not exercised by this test")
            }
            fn read_segment(
                &self,
                _id: SegmentId,
            ) -> impl std::future::Future<
                Output = Result<Vec<arrow::record_batch::RecordBatch>, PhotonError>,
            > + Send {
                async move { unimplemented!("FakeWal::read_segment is not exercised by this test") }
            }
            fn remove_segment(&self, _id: SegmentId) -> Result<(), PhotonError> {
                unimplemented!("FakeWal::remove_segment is not exercised by this test")
            }
        }

        let state = HttpState {
            wal: Arc::new(FakeWal),
            token: "t".to_string(),
            schema: LogSchema::new(&["service.name".to_string()]),
            in_flight: Arc::new(Semaphore::new(1)),
            counters: Arc::new(photon_core::ingest_counters::IngestCounters::new()),
        };

        let _first_permit = state
            .in_flight
            .clone()
            .try_acquire_owned()
            .expect("first permit should be available with max_in_flight = 1");

        let second_permit = state.in_flight.clone().try_acquire_owned();
        assert!(
            second_permit.is_err(),
            "second concurrent permit should be refused while the first is held"
        );
    }
}

/// Drives `ingest_logs` end-to-end with a real (always-succeeding) WAL fake and asserts the
/// shared `IngestCounters.logs` tally advances by the number of records in the request — this
/// is what lets `/api/usage/series` report ingest rates without touching the WAL/compactor.
#[cfg(test)]
mod counter_tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::logs::v1::{
        LogRecord as OtlpLogRecord, ResourceLogs, ScopeLogs,
    };
    use photon_core::segment::SegmentId;

    struct FakeWal;

    #[allow(clippy::manual_async_fn)]
    impl Wal for FakeWal {
        fn append(
            &self,
            _batch: arrow::record_batch::RecordBatch,
        ) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { Ok(()) }
        }
        fn sync(&self) -> impl std::future::Future<Output = Result<(), PhotonError>> + Send {
            async move { unimplemented!("FakeWal::sync is not exercised by this test") }
        }
        fn list_closed_segments(&self) -> Result<Vec<SegmentId>, PhotonError> {
            unimplemented!("FakeWal::list_closed_segments is not exercised by this test")
        }
        fn read_segment(
            &self,
            _id: SegmentId,
        ) -> impl std::future::Future<
            Output = Result<Vec<arrow::record_batch::RecordBatch>, PhotonError>,
        > + Send {
            async move { unimplemented!("FakeWal::read_segment is not exercised by this test") }
        }
        fn remove_segment(&self, _id: SegmentId) -> Result<(), PhotonError> {
            unimplemented!("FakeWal::remove_segment is not exercised by this test")
        }
    }

    /// Encode an `ExportLogsServiceRequest` carrying `n` bare log records into protobuf bytes.
    fn logs_body(n: usize) -> Bytes {
        let log_record = OtlpLogRecord {
            time_unix_nano: 1,
            observed_time_unix_nano: 0,
            severity_number: 0,
            severity_text: String::new(),
            body: None,
            attributes: vec![],
            dropped_attributes_count: 0,
            flags: 0,
            trace_id: vec![],
            span_id: vec![],
        };
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: std::iter::repeat_n(log_record, n).collect(),
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let mut buf = Vec::new();
        <ExportLogsServiceRequest as prost::Message>::encode(&req, &mut buf)
            .expect("encode never fails for a valid message");
        Bytes::from(buf)
    }

    #[tokio::test]
    async fn ingest_logs_advances_the_logs_counter() {
        let counters = Arc::new(IngestCounters::new());
        let state = Arc::new(HttpState {
            wal: Arc::new(FakeWal),
            token: "t".to_string(),
            schema: LogSchema::new(&["service.name".to_string()]),
            in_flight: Arc::new(Semaphore::new(4)),
            counters: counters.clone(),
        });

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer t".parse().unwrap(),
        );

        let resp = ingest_logs::<FakeWal>(State(state), headers, logs_body(2)).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let (rows, _bytes) = counters.logs.snapshot();
        assert_eq!(rows, 2);
    }
}
