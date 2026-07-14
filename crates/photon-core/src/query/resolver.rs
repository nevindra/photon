//! Resolve grammar field names against the schema (fixed columns + configured promoted
//! attributes) and specialize `level` / body. Long-tail (non-promoted) attribute names are
//! supported: any name that is neither a fixed column nor a promoted attribute resolves to
//! `FieldRef::MapAttr`, read from the `attributes` Map column at query time.

use std::collections::HashSet;

use super::ast::{Cmp, Query, TermKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ResolveError {
    pub message: String,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Which concrete string-valued field a grammar name maps to. `severity_number` (via
/// `Level`) and body (via `FreeText`) are handled specially and never become a `FieldRef`.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldRef {
    TraceId,
    SpanId,
    SeverityText,
    /// A promoted attribute column (e.g. `service.name`, `host.name`). In Parquet it is a
    /// real column of that name; in a `LogRecord` it lives in `attributes[name]`.
    Attr(String),
    /// A long-tail (non-promoted) attribute. In Parquet it lives inside the `attributes`
    /// Map column (read via `get_field`); in a `LogRecord` it lives in `attributes[name]`.
    MapAttr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTerm {
    pub negated: bool,
    pub kind: ResolvedKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedKind {
    /// `level:error,warn` → `severity_number` in any of these inclusive ranges.
    Level {
        ranges: Vec<(i32, i32)>,
    },
    Match {
        field: FieldRef,
        values: Vec<String>,
    },
    Exists {
        field: FieldRef,
    },
    Compare {
        field: FieldRef,
        op: Cmp,
        value: f64,
    },
    FreeText {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedQuery {
    pub terms: Vec<ResolvedTerm>,
}

pub struct FieldResolver {
    promoted: HashSet<String>,
}

impl FieldResolver {
    pub fn new(promoted: &[String]) -> FieldResolver {
        FieldResolver {
            promoted: promoted.iter().cloned().collect(),
        }
    }

    pub fn resolve(&self, query: &Query) -> Result<ResolvedQuery, ResolveError> {
        let mut terms = Vec::with_capacity(query.terms.len());
        for term in &query.terms {
            terms.push(ResolvedTerm {
                negated: term.negated,
                kind: self.resolve_kind(&term.kind)?,
            });
        }
        Ok(ResolvedQuery { terms })
    }

    fn resolve_kind(&self, kind: &TermKind) -> Result<ResolvedKind, ResolveError> {
        match kind {
            TermKind::FreeText { text } => Ok(ResolvedKind::FreeText { text: text.clone() }),
            TermKind::Match { field, values } if field == "level" => Ok(ResolvedKind::Level {
                ranges: level_ranges(values)?,
            }),
            TermKind::Match { field, values } => Ok(ResolvedKind::Match {
                field: self.resolve_field(field)?,
                values: values.clone(),
            }),
            TermKind::Exists { field } if field == "level" => Err(ResolveError {
                message: "`level:*` is not supported; filter by a level keyword".into(),
            }),
            TermKind::Exists { field } => Ok(ResolvedKind::Exists {
                field: self.resolve_field(field)?,
            }),
            TermKind::Compare { field, .. } if field == "level" => Err(ResolveError {
                message: "numeric comparison on `level` is not supported; use level keywords"
                    .into(),
            }),
            TermKind::Compare { field, op, value } => Ok(ResolvedKind::Compare {
                field: self.resolve_field(field)?,
                op: *op,
                value: *value,
            }),
        }
    }

    fn resolve_field(&self, name: &str) -> Result<FieldRef, ResolveError> {
        match name {
            "service" => Ok(FieldRef::Attr("service.name".into())),
            "trace_id" => Ok(FieldRef::TraceId),
            "span_id" => Ok(FieldRef::SpanId),
            "severity_text" => Ok(FieldRef::SeverityText),
            "body" => Err(ResolveError {
                message: "use quotes or bare words to search the body, e.g. \"timeout\"".into(),
            }),
            other if self.promoted.contains(other) => Ok(FieldRef::Attr(other.to_string())),
            other => Ok(FieldRef::MapAttr(other.to_string())),
        }
    }

    /// Resolve one bare field name to a `FieldRef` (same rules as grammar field resolution).
    /// Used by faceting, which groups by a single field. `level` is rejected (it is a severity
    /// bucket, not a stored field — the UI facets on `severity_text`), and `body` is rejected by
    /// `resolve_field` (free-text, not a field).
    pub fn resolve_field_name(&self, name: &str) -> Result<FieldRef, ResolveError> {
        if name == "level" {
            return Err(ResolveError {
                message: "`level` is a severity bucket, not a facetable field".into(),
            });
        }
        self.resolve_field(name)
    }
}

/// Level keywords → inclusive `severity_number` ranges (same buckets photon-api uses).
fn level_ranges(values: &[String]) -> Result<Vec<(i32, i32)>, ResolveError> {
    values
        .iter()
        .map(|v| match v.as_str() {
            "debug" => Ok((1, 8)),
            "info" => Ok((9, 12)),
            "warn" => Ok((13, 16)),
            "error" => Ok((17, 20)),
            "fatal" => Ok((21, 24)),
            other => Err(ResolveError {
                message: format!("unknown level `{other}`"),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::ast::Cmp;
    use crate::query::parse;

    fn resolver() -> FieldResolver {
        FieldResolver::new(&[
            "service.name".to_string(),
            "host.name".to_string(),
            "status_code".to_string(),
        ])
    }

    fn resolve(input: &str) -> Result<ResolvedQuery, ResolveError> {
        resolver().resolve(&parse(input).unwrap())
    }

    #[test]
    fn service_alias_and_promoted_and_fixed() {
        assert_eq!(
            resolve("service:api").unwrap().terms,
            vec![ResolvedTerm {
                negated: false,
                kind: ResolvedKind::Match {
                    field: FieldRef::Attr("service.name".into()),
                    values: vec!["api".into()]
                }
            }]
        );
        assert_eq!(
            resolve("host.name:*").unwrap().terms[0].kind,
            ResolvedKind::Exists {
                field: FieldRef::Attr("host.name".into())
            }
        );
        assert_eq!(
            resolve("trace_id:abc").unwrap().terms[0].kind,
            ResolvedKind::Match {
                field: FieldRef::TraceId,
                values: vec!["abc".into()]
            }
        );
    }

    #[test]
    fn level_maps_to_ranges() {
        assert_eq!(
            resolve("level:error,warn").unwrap().terms[0].kind,
            ResolvedKind::Level {
                ranges: vec![(17, 20), (13, 16)]
            }
        );
    }

    #[test]
    fn compare_and_freetext() {
        assert_eq!(
            resolve("status_code>=500").unwrap().terms[0].kind,
            ResolvedKind::Compare {
                field: FieldRef::Attr("status_code".into()),
                op: Cmp::Ge,
                value: 500.0
            }
        );
        assert_eq!(
            resolve("\"boom\"").unwrap().terms[0].kind,
            ResolvedKind::FreeText {
                text: "boom".into()
            }
        );
    }

    #[test]
    fn maptattr_body_and_bad_level() {
        assert_eq!(
            resolve("region:us").unwrap().terms[0].kind,
            ResolvedKind::Match {
                field: FieldRef::MapAttr("region".into()),
                values: vec!["us".into()]
            }
        );
        assert!(resolve("body:x").is_err()); // must use quotes/bare words
        assert!(resolve("level:nope").is_err()); // unknown level keyword
    }
}
