//! Metrics label-matcher resolver — sibling of `FieldResolver` (logs) and `SpanFieldResolver`
//! (spans). The metric name, aggregation, group-by, and time-bucketing are structured builder
//! controls, NOT grammar; the grammar covers only label matchers on a metric's attributes.
//! Metrics have no body/name text column, so a `FreeText` term is a resolve error.

use std::collections::HashSet;

use crate::query::ast::{Cmp, Query, TermKind};
use crate::query::resolver::ResolveError;

/// A resolved metrics field: a real column reference the two compilers turn into SQL / in-memory
/// lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricFieldRef {
    /// The `scope_name` fixed column.
    ScopeName,
    /// A promoted attribute with its own Utf8 column (includes `service.name`).
    Attr(String),
    /// A long-tail attribute read from the `attributes` Map at query time.
    MapAttr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetricResolvedKind {
    Match {
        field: MetricFieldRef,
        values: Vec<String>,
    },
    Exists {
        field: MetricFieldRef,
    },
    Compare {
        field: MetricFieldRef,
        op: Cmp,
        value: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricResolvedTerm {
    pub negated: bool,
    pub kind: MetricResolvedKind,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MetricResolvedQuery {
    pub terms: Vec<MetricResolvedTerm>,
}

pub struct MetricFieldResolver {
    promoted: HashSet<String>,
}

impl MetricFieldResolver {
    pub fn new(promoted: &[String]) -> MetricFieldResolver {
        MetricFieldResolver {
            promoted: promoted.iter().cloned().collect(),
        }
    }

    /// Map a label name to its column. `service` aliases the promoted `service.name`; `scope`
    /// and `scope_name` alias the `scope_name` column; a name in the promoted set is its own
    /// column; anything else is a map attribute.
    pub fn resolve_field_name(&self, name: &str) -> Result<MetricFieldRef, ResolveError> {
        Ok(match name {
            "service" => MetricFieldRef::Attr("service.name".to_string()),
            "scope" | "scope_name" => MetricFieldRef::ScopeName,
            other if self.promoted.contains(other) => MetricFieldRef::Attr(other.to_string()),
            other => MetricFieldRef::MapAttr(other.to_string()),
        })
    }

    pub fn resolve(&self, q: &Query) -> Result<MetricResolvedQuery, ResolveError> {
        let mut terms = Vec::with_capacity(q.terms.len());
        for t in &q.terms {
            let kind =
                match &t.kind {
                    TermKind::Match { field, values } => MetricResolvedKind::Match {
                        field: self.resolve_field_name(field)?,
                        values: values.clone(),
                    },
                    TermKind::Exists { field } => MetricResolvedKind::Exists {
                        field: self.resolve_field_name(field)?,
                    },
                    TermKind::Compare { field, op, value } => MetricResolvedKind::Compare {
                        field: self.resolve_field_name(field)?,
                        op: *op,
                        value: *value,
                    },
                    TermKind::FreeText { .. } => return Err(ResolveError {
                        message:
                            "free-text search is not supported for metrics; use field:value filters"
                                .to_string(),
                    }),
                };
            terms.push(MetricResolvedTerm {
                negated: t.negated,
                kind,
            });
        }
        Ok(MetricResolvedQuery { terms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parser::parse;

    fn resolver() -> MetricFieldResolver {
        MetricFieldResolver::new(&["service.name".to_string(), "http.route".to_string()])
    }

    #[test]
    fn service_alias_and_promoted_and_map() {
        let rq = resolver()
            .resolve(&parse("service:checkout").unwrap())
            .unwrap();
        assert!(matches!(
            &rq.terms[0].kind,
            MetricResolvedKind::Match { field: MetricFieldRef::Attr(n), .. } if n == "service.name"
        ));
        let rq = resolver()
            .resolve(&parse("http.route:/pay").unwrap())
            .unwrap();
        assert!(matches!(
            &rq.terms[0].kind,
            MetricResolvedKind::Match { field: MetricFieldRef::Attr(n), .. } if n == "http.route"
        ));
        let rq = resolver()
            .resolve(&parse("deployment:prod").unwrap())
            .unwrap();
        assert!(matches!(
            &rq.terms[0].kind,
            MetricResolvedKind::Match { field: MetricFieldRef::MapAttr(n), .. } if n == "deployment"
        ));
    }

    #[test]
    fn scope_alias_exists_negate_compare() {
        let rq = resolver()
            .resolve(&parse("scope:otel.sdk").unwrap())
            .unwrap();
        assert!(matches!(
            rq.terms[0].kind,
            MetricResolvedKind::Match {
                field: MetricFieldRef::ScopeName,
                ..
            }
        ));
        let rq = resolver().resolve(&parse("http.route:*").unwrap()).unwrap();
        assert!(matches!(
            rq.terms[0].kind,
            MetricResolvedKind::Exists { .. }
        ));
        let rq = resolver().resolve(&parse("-service:db").unwrap()).unwrap();
        assert!(rq.terms[0].negated);
        let rq = resolver()
            .resolve(&parse("http.status_code>=500").unwrap())
            .unwrap();
        assert!(matches!(
            rq.terms[0].kind,
            MetricResolvedKind::Compare { op: Cmp::Ge, .. }
        ));
    }

    #[test]
    fn free_text_is_rejected() {
        let err = resolver()
            .resolve(&parse("some bare words").unwrap())
            .unwrap_err();
        assert!(err.message.contains("free-text"));
    }
}
