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

impl MetricResolvedQuery {
    /// The exact `tenant` values a matching row must have, when the query pins one.
    ///
    /// A federation central stamps `tenant` on every row it accepts, and the UI's Site picker
    /// puts a `tenant:<name>` term on every query, so this is the dimension a central actually
    /// filters by. The engine keeps a segment only when its `FileEntry::tenant_range` overlaps
    /// one of these values — a manifest-only check, no sidecar read.
    ///
    /// `None` means the query says nothing about `tenant`, so every file is kept. Only un-negated
    /// `Match` terms count: `-tenant:x` is *satisfied* by a row with no tenant at all, and
    /// `tenant:*` is an existence test rather than a value. Terms are AND-ed, so the first such
    /// term is a genuine requirement and pruning on it alone is sound.
    pub fn tenant_values(&self) -> Option<&[String]> {
        self.terms.iter().find_map(|t| {
            if t.negated {
                return None;
            }
            let MetricResolvedKind::Match { field, values } = &t.kind else {
                return None;
            };
            let (MetricFieldRef::Attr(k) | MetricFieldRef::MapAttr(k)) = field else {
                return None;
            };
            (k == crate::TENANT_ATTRIBUTE && !values.is_empty()).then_some(values.as_slice())
        })
    }
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
    fn tenant_values_reads_the_pinned_sites() {
        // Metrics matter here because the cheapest way to run a central is
        // `[federation].signals = ["metrics"]` plus the service rollup.
        let promoted =
            MetricFieldResolver::new(&["service.name".to_string(), "tenant".to_string()]);
        let q = |s: &str| promoted.resolve(&parse(s).unwrap()).unwrap();
        assert_eq!(
            q("tenant:jkt").tenant_values(),
            Some(&["jkt".to_string()][..])
        );
        assert_eq!(
            q("tenant:jkt,sgp").tenant_values(),
            Some(&["jkt".to_string(), "sgp".to_string()][..])
        );
        assert_eq!(
            resolver()
                .resolve(&parse("tenant:jkt").unwrap())
                .unwrap()
                .tenant_values(),
            Some(&["jkt".to_string()][..]),
            "the long-tail spelling resolves the same"
        );
        assert_eq!(q("service:api").tenant_values(), None);
        assert_eq!(q("-tenant:jkt").tenant_values(), None);
        assert_eq!(q("tenant:*").tenant_values(), None);
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
