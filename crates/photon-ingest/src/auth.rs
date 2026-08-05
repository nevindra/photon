//! Bearer-token check shared by the gRPC and HTTP receivers.
//!
//! Factored out as a pure function so the auth logic is unit-testable without spinning up
//! a live server (gRPC metadata and HTTP headers both reduce to `Option<&str>` + the
//! expected token).

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::metric::Data;
use opentelemetry_proto::tonic::resource::v1::Resource;
use photon_core::TenantTokenMap;
use subtle::ConstantTimeEq;

/// The resource attribute central stamps on every batch it accepts from a tenant token.
pub(crate) const TENANT_KEY: &str = "tenant";

/// Outcome of resolving a bearer header: the node's own ingest token, a registered tenant
/// token (federation), or neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Auth {
    Local,
    Tenant(String),
    Denied,
}

/// Resolve a bearer header against the local ingest token and the tenant token map. The local
/// token keeps its constant-time compare; tenant tokens are an exact `HashMap` lookup.
pub(crate) fn resolve_bearer(
    header_value: Option<&str>,
    local: &str,
    tenants: &TenantTokenMap,
) -> Auth {
    if check_bearer_token(header_value, local) {
        return Auth::Local;
    }
    let Some(token) = header_value.and_then(|v| v.strip_prefix("Bearer ")) else {
        return Auth::Denied;
    };
    // `into_inner` on poison: a panic elsewhere while holding the lock must not permanently
    // turn every tenant token into `Denied` — the map itself is never left half-written
    // (the registry replaces the whole HashMap under the write lock).
    let map = tenants.read().unwrap_or_else(|e| e.into_inner());
    match map.get(token).cloned() {
        Some(tenant) => Auth::Tenant(tenant),
        None => Auth::Denied,
    }
}

/// Insert-or-overwrite the `tenant` resource attribute. Client-supplied values never survive.
pub(crate) fn stamp_tenant(attrs: &mut Vec<KeyValue>, tenant: &str) {
    attrs.retain(|a| a.key != TENANT_KEY);
    attrs.push(KeyValue {
        key: TENANT_KEY.to_string(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(tenant.to_string())),
        }),
    });
}

fn stamp_resource(resource: &mut Option<Resource>, tenant: &str) {
    match resource {
        Some(res) => stamp_tenant(&mut res.attributes, tenant),
        None => {
            let mut attributes = Vec::with_capacity(1);
            stamp_tenant(&mut attributes, tenant);
            *resource = Some(Resource {
                attributes,
                dropped_attributes_count: 0,
            });
        }
    }
}

/// Record/span/data-point attributes beat resource attributes in every mapper's merge, so a
/// spoofed per-row `tenant` would outrank the stamp — drop it.
fn strip_tenant(attrs: &mut Vec<KeyValue>) {
    attrs.retain(|a| a.key != TENANT_KEY);
}

pub(crate) fn stamp_tenant_logs(req: &mut ExportLogsServiceRequest, tenant: &str) {
    for rl in &mut req.resource_logs {
        stamp_resource(&mut rl.resource, tenant);
        for sl in &mut rl.scope_logs {
            for lr in &mut sl.log_records {
                strip_tenant(&mut lr.attributes);
            }
        }
    }
}

pub(crate) fn stamp_tenant_traces(req: &mut ExportTraceServiceRequest, tenant: &str) {
    for rs in &mut req.resource_spans {
        stamp_resource(&mut rs.resource, tenant);
        for ss in &mut rs.scope_spans {
            for span in &mut ss.spans {
                strip_tenant(&mut span.attributes);
            }
        }
    }
}

pub(crate) fn stamp_tenant_metrics(req: &mut ExportMetricsServiceRequest, tenant: &str) {
    for rm in &mut req.resource_metrics {
        stamp_resource(&mut rm.resource, tenant);
        for sm in &mut rm.scope_metrics {
            for metric in &mut sm.metrics {
                match metric.data.as_mut() {
                    Some(Data::Gauge(g)) => g
                        .data_points
                        .iter_mut()
                        .for_each(|dp| strip_tenant(&mut dp.attributes)),
                    Some(Data::Sum(s)) => s
                        .data_points
                        .iter_mut()
                        .for_each(|dp| strip_tenant(&mut dp.attributes)),
                    Some(Data::Histogram(h)) => h
                        .data_points
                        .iter_mut()
                        .for_each(|dp| strip_tenant(&mut dp.attributes)),
                    Some(Data::ExponentialHistogram(h)) => h
                        .data_points
                        .iter_mut()
                        .for_each(|dp| strip_tenant(&mut dp.attributes)),
                    Some(Data::Summary(s)) => s
                        .data_points
                        .iter_mut()
                        .for_each(|dp| strip_tenant(&mut dp.attributes)),
                    None => {}
                }
            }
        }
    }
}

/// Returns `true` iff `header_value` is exactly `Bearer <token>`.
///
/// The token equality check is constant-time (`subtle::ConstantTimeEq`) so a byte-by-byte
/// `==` scan can't leak a timing side-channel on the shared ingest secret. Everything else
/// (missing header, missing/incorrect `Bearer ` scheme) is not secret-dependent and stays a
/// plain, short-circuiting comparison.
pub(crate) fn check_bearer_token(header_value: Option<&str>, token: &str) -> bool {
    match header_value {
        Some(v) => v
            .strip_prefix("Bearer ")
            .is_some_and(|t| bool::from(t.as_bytes().ct_eq(token.as_bytes()))),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(value.to_string())),
            }),
        }
    }

    fn string_value(kv: &KeyValue) -> &str {
        match kv.value.as_ref().and_then(|v| v.value.as_ref()) {
            Some(Value::StringValue(s)) => s,
            other => panic!("expected a string value, got {other:?}"),
        }
    }

    #[test]
    fn resolve_local_token() {
        let map = TenantTokenMap::default();
        assert!(matches!(
            resolve_bearer(Some("Bearer secret"), "secret", &map),
            Auth::Local
        ));
    }

    #[test]
    fn resolve_tenant_token() {
        let map = TenantTokenMap::default();
        map.write()
            .unwrap()
            .insert("tk_tenant_x".into(), "divtik".into());
        match resolve_bearer(Some("Bearer tk_tenant_x"), "secret", &map) {
            Auth::Tenant(t) => assert_eq!(t, "divtik"),
            other => panic!("expected tenant, got {other:?}"),
        }
    }

    #[test]
    fn resolve_denies_unknown_and_missing() {
        let map = TenantTokenMap::default();
        assert!(matches!(
            resolve_bearer(Some("Bearer nope"), "secret", &map),
            Auth::Denied
        ));
        assert!(matches!(resolve_bearer(None, "secret", &map), Auth::Denied));
    }

    #[test]
    fn stamp_tenant_overwrites_client_label() {
        let mut attrs = vec![kv("tenant", "spoofed"), kv("service.name", "api")];
        stamp_tenant(&mut attrs, "divtik");
        let vals: Vec<_> = attrs.iter().filter(|a| a.key == "tenant").collect();
        assert_eq!(vals.len(), 1);
        assert_eq!(string_value(vals[0]), "divtik");
    }

    /// Trust boundary: a client-supplied `tenant` on the *record* wins over the resource
    /// attribute in the mapper's merge, so per-record labels must be stripped too.
    #[test]
    fn stamp_tenant_logs_strips_record_level_label() {
        use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
        use opentelemetry_proto::tonic::logs::v1::{
            LogRecord as OtlpLogRecord, ResourceLogs, ScopeLogs,
        };
        use opentelemetry_proto::tonic::resource::v1::Resource;

        let mut req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![kv("tenant", "spoofed")],
                    dropped_attributes_count: 0,
                }),
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![OtlpLogRecord {
                        attributes: vec![kv("tenant", "spoofed"), kv("http.method", "GET")],
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        stamp_tenant_logs(&mut req, "divtik");

        let rl = &req.resource_logs[0];
        let res_attrs = &rl.resource.as_ref().unwrap().attributes;
        let stamped: Vec<_> = res_attrs.iter().filter(|a| a.key == "tenant").collect();
        assert_eq!(stamped.len(), 1);
        assert_eq!(string_value(stamped[0]), "divtik");
        let rec_attrs = &rl.scope_logs[0].log_records[0].attributes;
        assert!(rec_attrs.iter().all(|a| a.key != "tenant"));
        assert_eq!(rec_attrs.len(), 1);
    }

    /// A resource-less group still gets a stamped tenant (otherwise the row would be untagged).
    #[test]
    fn stamp_tenant_logs_creates_missing_resource() {
        use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
        use opentelemetry_proto::tonic::logs::v1::{ResourceLogs, ScopeLogs};

        let mut req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs::default()],
                schema_url: String::new(),
            }],
        };
        stamp_tenant_logs(&mut req, "divtik");
        let attrs = &req.resource_logs[0].resource.as_ref().unwrap().attributes;
        assert_eq!(attrs.len(), 1);
        assert_eq!(string_value(&attrs[0]), "divtik");
    }

    #[test]
    fn correct_bearer_token_passes() {
        assert!(check_bearer_token(Some("Bearer good"), "good"));
    }

    #[test]
    fn wrong_token_fails() {
        assert!(!check_bearer_token(Some("Bearer bad"), "good"));
    }

    #[test]
    fn missing_header_fails() {
        assert!(!check_bearer_token(None, "good"));
    }

    #[test]
    fn missing_bearer_prefix_fails() {
        assert!(!check_bearer_token(Some("good"), "good"));
    }

    #[test]
    fn case_sensitive_scheme_fails() {
        assert!(!check_bearer_token(Some("bearer good"), "good"));
    }

    #[test]
    fn empty_expected_token_still_requires_prefix() {
        assert!(check_bearer_token(Some("Bearer "), ""));
        assert!(!check_bearer_token(Some(""), ""));
    }
}
