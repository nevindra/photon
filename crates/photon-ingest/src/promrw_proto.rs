//! Hand-written prost structs for the Prometheus remote-write 1.0 wire format
//! (`prometheus.WriteRequest`). Photon has no proto-codegen build step — OTLP types come from
//! the `opentelemetry-proto` crate — and the RW 1.0 schema is tiny and frozen, so the four
//! messages are transcribed directly against the existing `prost` dependency.
//!
//! Wire reference (Prometheus `prompb/remote.proto` + `types.proto`):
//! ```proto
//! message WriteRequest { repeated TimeSeries timeseries = 1; /* metadata = 3, ignored */ }
//! message TimeSeries   { repeated Label labels = 1; repeated Sample samples = 2; }
//! message Label        { string name = 1; string value = 2; }
//! message Sample       { double value = 1; int64 timestamp = 2; } // timestamp = unix millis
//! ```
//! The `prost::Message` derive supplies `Default` and `Debug`, so they are not derived here.

#[derive(Clone, PartialEq, prost::Message)]
pub struct WriteRequest {
    #[prost(message, repeated, tag = "1")]
    pub timeseries: Vec<TimeSeries>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct TimeSeries {
    #[prost(message, repeated, tag = "1")]
    pub labels: Vec<Label>,
    #[prost(message, repeated, tag = "2")]
    pub samples: Vec<Sample>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Label {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Sample {
    #[prost(double, tag = "1")]
    pub value: f64,
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn write_request_prost_round_trips() {
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
                    value: 42.0,
                    timestamp: 1_700_000_000_000,
                }],
            }],
        };
        let bytes = req.encode_to_vec();
        let decoded = WriteRequest::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded, req);
    }
}
