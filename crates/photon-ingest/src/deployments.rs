//! Deploy detection at ingest: notice the first time a `(service, environment, tenant, version)`
//! shows up in a trace batch.
//!
//! A deployment is not a signal Photon receives — nobody sends one. But every service that follows
//! OTel semconv already stamps `service.version` on its resource, and the moment a version appears
//! that has never appeared before, something was deployed. That is the whole detector.
//!
//! **It reads the RESOURCE, not the spans.** `service.version` is a resource attribute per semconv
//! (`opentelemetry.io/docs/specs/semconv/resource/`), so a batch of 5,000 spans is four string
//! lookups, not 5,000. The plan said "on a span carrying an unseen `service.version`"; reading the
//! resource is the same fact for a fraction of the cost, and a span-level `service.version` would
//! be a non-conforming producer anyway.
//!
//! **Cost on the hot path is one hash lookup per resource group**, and the implementation behind
//! [`DeploymentWatcher`] short-circuits on an already-seen triple before touching any storage. A
//! service that never redeploys pays for one `HashSet::contains` per batch and nothing else.
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;

use crate::otlp_value::any_value_str;

/// The OTel semconv resource attributes this reads.
pub const SERVICE_NAME: &str = "service.name";
pub const SERVICE_VERSION: &str = "service.version";
pub const DEPLOYMENT_ENVIRONMENT: &str = "deployment.environment";
/// Stamped by `auth::stamp_tenant_traces` when a federated tenant token authenticated the batch.
pub const TENANT: &str = "tenant";

/// One `(service, environment, tenant, version)` sighting.
///
/// `environment` and `tenant` are empty strings, never `None`, when the resource does not carry
/// them: the store's uniqueness constraint is over all four, and SQLite treats NULL as distinct
/// from NULL, so a nullable column would let the same unversioned deployment insert forever.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SeenVersion {
    pub service: String,
    pub environment: String,
    pub tenant: String,
    pub version: String,
}

/// The seam `photon-server` implements over the deployments table. Sync on purpose: the
/// implementation answers an already-seen sighting from an in-memory set without any I/O, and the
/// rare miss is one small SQLite insert. Making this async would put a `.await` on the ingest path
/// for something that almost never does work.
pub trait DeploymentWatcher: Send + Sync {
    /// Record a sighting. Idempotent — the implementation drops one it has already seen.
    fn observe(&self, seen: &SeenVersion);
}

/// Every distinct versioned resource in a trace batch.
///
/// Pure: takes the decoded request, allocates only for resources that actually carry a version.
/// Duplicate resource groups within one batch collapse, so a collector that splits one service
/// across ten groups still produces one sighting.
pub fn resource_versions(req: &ExportTraceServiceRequest) -> Vec<SeenVersion> {
    let mut out: Vec<SeenVersion> = Vec::new();
    for rs in &req.resource_spans {
        let Some(resource) = rs.resource.as_ref() else {
            continue;
        };
        let mut service = "";
        let mut version = "";
        let mut environment = "";
        let mut tenant = "";
        for kv in &resource.attributes {
            let Some(v) = kv.value.as_ref().and_then(any_value_str) else {
                continue;
            };
            match kv.key.as_str() {
                SERVICE_NAME => service = v,
                SERVICE_VERSION => version = v,
                DEPLOYMENT_ENVIRONMENT => environment = v,
                TENANT => tenant = v,
                _ => {}
            }
        }
        // A version with no service names nothing deployable, and a service with no version says
        // only that the producer does not report one — neither is a deployment.
        if service.is_empty() || version.is_empty() {
            continue;
        }
        let seen = SeenVersion {
            service: service.to_string(),
            environment: environment.to_string(),
            tenant: tenant.to_string(),
            version: version.to_string(),
        };
        if !out.contains(&seen) {
            out.push(seen);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    fn kv(k: &str, v: &str) -> KeyValue {
        KeyValue {
            key: k.into(),
            value: Some(AnyValue {
                value: Some(Value::StringValue(v.into())),
            }),
        }
    }

    fn req(groups: Vec<Vec<KeyValue>>) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: groups
                .into_iter()
                .map(|attributes| ResourceSpans {
                    resource: Some(Resource {
                        attributes,
                        dropped_attributes_count: 0,
                    }),
                    scope_spans: vec![ScopeSpans {
                        spans: vec![Span::default()],
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_versioned_resource_is_one_sighting() {
        let v = resource_versions(&req(vec![vec![
            kv("service.name", "api"),
            kv("service.version", "1.4.2"),
            kv("deployment.environment", "prod"),
        ]]));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].service, "api");
        assert_eq!(v[0].version, "1.4.2");
        assert_eq!(v[0].environment, "prod");
        assert_eq!(v[0].tenant, "");
    }

    #[test]
    fn an_unversioned_resource_is_not_a_deployment() {
        // The overwhelmingly common case: a producer that simply does not report a version.
        let v = resource_versions(&req(vec![vec![kv("service.name", "api")]]));
        assert!(v.is_empty());
    }

    #[test]
    fn a_version_with_no_service_names_nothing_deployable() {
        let v = resource_versions(&req(vec![vec![kv("service.version", "1.0.0")]]));
        assert!(v.is_empty());
    }

    #[test]
    fn duplicate_resource_groups_collapse_to_one_sighting() {
        let g = vec![kv("service.name", "api"), kv("service.version", "1.4.2")];
        let v = resource_versions(&req(vec![g.clone(), g.clone(), g]));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn two_environments_of_one_version_are_two_sightings() {
        // Same build in staging and prod is two deployments — they happened at different times
        // and one can be rolled back without the other.
        let v = resource_versions(&req(vec![
            vec![
                kv("service.name", "api"),
                kv("service.version", "1.4.2"),
                kv("deployment.environment", "staging"),
            ],
            vec![
                kv("service.name", "api"),
                kv("service.version", "1.4.2"),
                kv("deployment.environment", "prod"),
            ],
        ]));
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn a_tenant_stamp_is_carried_into_the_sighting() {
        // Central aggregates many Sites; the same version deployed at two Sites is two deploys.
        let v = resource_versions(&req(vec![vec![
            kv("service.name", "api"),
            kv("service.version", "1.4.2"),
            kv("tenant", "jkt"),
        ]]));
        assert_eq!(v[0].tenant, "jkt");
    }

    #[test]
    fn a_non_string_version_attribute_is_ignored() {
        let v = resource_versions(&req(vec![vec![
            kv("service.name", "api"),
            KeyValue {
                key: "service.version".into(),
                value: Some(AnyValue {
                    value: Some(Value::IntValue(142)),
                }),
            },
        ]]));
        assert!(v.is_empty());
    }

    #[test]
    fn a_batch_with_no_resource_yields_nothing() {
        let req = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: None,
                scope_spans: vec![],
                schema_url: String::new(),
            }],
        };
        assert!(resource_versions(&req).is_empty());
    }

    #[test]
    fn an_empty_batch_yields_nothing() {
        assert!(resource_versions(&ExportTraceServiceRequest::default()).is_empty());
    }
}
