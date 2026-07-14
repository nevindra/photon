//! OTLP log payload construction + the logs [`Payload`] impl. No I/O, so payload shape is
//! unit-testable and — via a round-trip through `photon_ingest::otlp_logs_to_records` — verified
//! to map the way the real receiver maps it.
//!
//! A batch is spread across a handful of `ResourceLogs` groups so `service.name` / `host.name`
//! (both *promoted* columns, and `service.name` the primary sort key) actually vary — otherwise
//! skip-index min/max pruning and token blooms would be exercised trivially or not at all.

use crate::payload::{Built, Payload};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{
    any_value::Value, AnyValue, InstrumentationScope, KeyValue,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs, SeverityNumber};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use rand::rngs::SmallRng;
use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};

/// The logs load source: `batch` records per request across up to `services` resource groups.
pub struct LogsPayload {
    pub batch: usize,
    pub services: usize,
}

impl Payload for LogsPayload {
    fn cost(&self) -> f64 {
        self.batch as f64
    }

    fn build(&self, rng: &mut SmallRng) -> Built {
        let body = build_batch(self.batch, self.services, rng);
        Built {
            body,
            units: self.batch as u64,
            spans: 0,
        }
    }
}

const HOSTS: &[&str] = &["host-a", "host-b", "host-c", "host-d"];
const SCOPES: &[&str] = &["http.server", "db.pool", "auth", "scheduler"];
const ENVS: &[&str] = &["prod", "staging"];
const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE"];
const STATUSES: &[u16] = &[200, 200, 200, 200, 201, 404, 500, 503];
const BODIES: &[&str] = &[
    "request completed",
    "cache miss for key",
    "connection established",
    "query executed",
    "retrying downstream call",
    "user authenticated",
    "payload rejected: invalid schema",
    "background job finished",
];
/// (severity number, text, selection weight). Mostly INFO, a long tail of the rest.
const SEVERITIES: &[(SeverityNumber, &str, u32)] = &[
    (SeverityNumber::Info, "INFO", 70),
    (SeverityNumber::Debug, "DEBUG", 15),
    (SeverityNumber::Warn, "WARN", 10),
    (SeverityNumber::Error, "ERROR", 5),
];

fn str_value(s: impl Into<String>) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(Value::StringValue(s.into())),
    })
}

fn kv(key: &str, val: impl Into<String>) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: str_value(val),
    }
}

fn pick_severity(rng: &mut impl Rng) -> (SeverityNumber, &'static str) {
    let total: u32 = SEVERITIES.iter().map(|s| s.2).sum();
    let mut roll = rng.gen_range(0..total);
    for (num, text, weight) in SEVERITIES {
        if roll < *weight {
            return (*num, text);
        }
        roll -= *weight;
    }
    (SeverityNumber::Info, "INFO")
}

fn build_record(now_nanos: u64, rng: &mut impl Rng) -> LogRecord {
    let (severity_number, severity_text) = pick_severity(rng);
    let body = BODIES[rng.gen_range(0..BODIES.len())];
    let latency_ms = rng.gen_range(1..500);
    let status = STATUSES[rng.gen_range(0..STATUSES.len())];

    let attributes = vec![
        kv("http.method", METHODS[rng.gen_range(0..METHODS.len())]),
        kv("http.status_code", status.to_string()),
        kv("thread.id", rng.gen_range(1..64).to_string()),
    ];

    // Attach trace/span ids to a fraction of records, like a real correlated workload.
    let (trace_id, span_id) = if rng.gen_bool(0.3) {
        let mut trace = vec![0u8; 16];
        let mut span = vec![0u8; 8];
        rng.fill(&mut trace[..]);
        rng.fill(&mut span[..]);
        (trace, span)
    } else {
        (Vec::new(), Vec::new())
    };

    LogRecord {
        time_unix_nano: now_nanos,
        observed_time_unix_nano: now_nanos,
        severity_number: severity_number as i32,
        severity_text: severity_text.to_string(),
        body: str_value(format!("{body} ({latency_ms} ms)")),
        attributes,
        trace_id,
        span_id,
        ..Default::default()
    }
}

/// Build one `ExportLogsServiceRequest` carrying `batch` records, grouped across up to
/// `services` resource groups.
pub fn build_request(
    batch: usize,
    services: usize,
    rng: &mut impl Rng,
) -> ExportLogsServiceRequest {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let groups = services.min(batch).max(1);
    let per_group = batch / groups;
    let remainder = batch % groups;

    let mut resource_logs = Vec::with_capacity(groups);
    for g in 0..groups {
        let service = format!("service-{}", rng.gen_range(0..services));
        let host = HOSTS[rng.gen_range(0..HOSTS.len())];
        let env = ENVS[rng.gen_range(0..ENVS.len())];
        let scope = SCOPES[rng.gen_range(0..SCOPES.len())];
        let count = per_group + if g < remainder { 1 } else { 0 };

        let mut records = Vec::with_capacity(count);
        for _ in 0..count {
            records.push(build_record(now_nanos, rng));
        }

        resource_logs.push(ResourceLogs {
            resource: Some(Resource {
                attributes: vec![
                    kv("service.name", service),
                    kv("host.name", host),
                    kv("service.version", "1.4.2"),
                    kv("deployment.environment", env),
                ],
                ..Default::default()
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: scope.to_string(),
                    ..Default::default()
                }),
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        });
    }

    ExportLogsServiceRequest { resource_logs }
}

/// Build a batch and return its prost-encoded bytes, ready to POST to `/v1/logs`.
pub fn build_batch(batch: usize, services: usize, rng: &mut impl Rng) -> Vec<u8> {
    let req = build_request(batch, services, rng);
    let mut buf = Vec::with_capacity(req.encoded_len());
    req.encode(&mut buf)
        .expect("prost encode into a Vec is infallible");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn decode(bytes: &[u8]) -> ExportLogsServiceRequest {
        ExportLogsServiceRequest::decode(bytes).expect("valid OTLP protobuf")
    }

    #[test]
    fn batch_decodes_and_maps_to_expected_record_count() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(1);
        let records = photon_ingest::otlp_logs_to_records(decode(&build_batch(50, 4, &mut rng)));

        assert_eq!(records.len(), 50);
        for r in &records {
            let svc = r
                .attributes
                .get("service.name")
                .expect("service.name present");
            assert!(svc.starts_with("service-"), "unexpected service {svc}");
            assert!(r.attributes.contains_key("host.name"));
            assert!(r.body.is_some());
            assert!(r.severity_number.is_some());
        }
    }

    #[test]
    fn covers_all_service_cardinality_over_many_batches() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(7);
        let mut seen = BTreeSet::new();
        for _ in 0..50 {
            for r in photon_ingest::otlp_logs_to_records(decode(&build_batch(200, 6, &mut rng))) {
                seen.insert(r.attributes.get("service.name").unwrap().clone());
            }
        }
        let expected: BTreeSet<String> = (0..6).map(|i| format!("service-{i}")).collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn severity_distribution_hits_multiple_buckets() {
        let mut rng = rand::rngs::SmallRng::seed_from_u64(3);
        let mut buckets = BTreeSet::new();
        for r in photon_ingest::otlp_logs_to_records(decode(&build_batch(500, 3, &mut rng))) {
            buckets.insert(r.severity_number.unwrap());
        }
        assert!(
            buckets.len() >= 2,
            "expected multiple severities, got {buckets:?}"
        );
    }

    // Pull SeedableRng into scope for the seed_from_u64 calls above.
    use rand::SeedableRng;
}
