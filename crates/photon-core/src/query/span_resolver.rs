//! Resolve grammar field names for the SPANS schema (a sibling of `resolver::FieldResolver`).
//! Reuses the shared parser/AST; specializes `status`/`kind` (keyword→code), `duration` (numeric),
//! and free-text (substring on the `name`/operation column). Long-tail attribute names resolve to
//! `SpanFieldRef::MapAttr`, read from the `attributes` Map at query time.

use std::collections::HashSet;

use super::ast::{Cmp, Query, TermKind};
use super::resolver::ResolveError;

#[derive(Debug, Clone, PartialEq)]
pub enum SpanFieldRef {
    TraceId,
    SpanId,
    ParentSpanId,
    Name,
    ScopeName,
    /// `duration_nanos` (Int64). Compare-only.
    Duration,
    /// A promoted attribute column (incl. `service.name`).
    Attr(String),
    /// A long-tail attribute, read from the `attributes` Map.
    MapAttr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpanResolvedKind {
    Status {
        codes: Vec<i32>,
    },
    Kind {
        codes: Vec<i32>,
    },
    Match {
        field: SpanFieldRef,
        values: Vec<String>,
    },
    Exists {
        field: SpanFieldRef,
    },
    Compare {
        field: SpanFieldRef,
        op: Cmp,
        value: f64,
    },
    FreeText {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpanResolvedTerm {
    pub negated: bool,
    pub kind: SpanResolvedKind,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpanResolvedQuery {
    pub terms: Vec<SpanResolvedTerm>,
}

pub struct SpanFieldResolver {
    promoted: HashSet<String>,
}

impl SpanFieldResolver {
    pub fn new(promoted: &[String]) -> SpanFieldResolver {
        SpanFieldResolver {
            promoted: promoted.iter().cloned().collect(),
        }
    }

    pub fn resolve(&self, query: &Query) -> Result<SpanResolvedQuery, ResolveError> {
        let mut terms = Vec::with_capacity(query.terms.len());
        for term in &query.terms {
            terms.push(SpanResolvedTerm {
                negated: term.negated,
                kind: self.resolve_kind(&term.kind)?,
            });
        }
        Ok(SpanResolvedQuery { terms })
    }

    fn resolve_kind(&self, kind: &TermKind) -> Result<SpanResolvedKind, ResolveError> {
        match kind {
            TermKind::FreeText { text } => Ok(SpanResolvedKind::FreeText { text: text.clone() }),

            TermKind::Match { field, values } if field == "status" => {
                Ok(SpanResolvedKind::Status {
                    codes: status_codes(values)?,
                })
            }
            TermKind::Match { field, values } if field == "kind" => Ok(SpanResolvedKind::Kind {
                codes: kind_codes(values)?,
            }),
            TermKind::Match { field, values } => Ok(SpanResolvedKind::Match {
                field: self.resolve_field(field)?,
                values: values.clone(),
            }),

            TermKind::Exists { field } if field == "status" || field == "kind" => {
                Err(ResolveError {
                    message: format!("`{field}:*` is not supported; filter by a {field} keyword"),
                })
            }
            TermKind::Exists { field } if field == "duration" => Err(ResolveError {
                message:
                    "`duration:*` is not supported; use a numeric compare like duration>=500ms"
                        .into(),
            }),
            TermKind::Exists { field } => Ok(SpanResolvedKind::Exists {
                field: self.resolve_field(field)?,
            }),

            TermKind::Compare { field, .. } if field == "status" || field == "kind" => {
                Err(ResolveError {
                    message: format!(
                        "numeric comparison on `{field}` is not supported; use keywords"
                    ),
                })
            }
            TermKind::Compare { field, op, value } => Ok(SpanResolvedKind::Compare {
                field: self.resolve_field(field)?,
                op: *op,
                value: *value,
            }),
        }
    }

    fn resolve_field(&self, name: &str) -> Result<SpanFieldRef, ResolveError> {
        match name {
            "service" => Ok(SpanFieldRef::Attr("service.name".into())),
            "operation" | "name" => Ok(SpanFieldRef::Name),
            "trace_id" => Ok(SpanFieldRef::TraceId),
            "span_id" => Ok(SpanFieldRef::SpanId),
            "parent_span_id" => Ok(SpanFieldRef::ParentSpanId),
            "scope" | "scope_name" => Ok(SpanFieldRef::ScopeName),
            "duration" => Ok(SpanFieldRef::Duration),
            other if self.promoted.contains(other) => Ok(SpanFieldRef::Attr(other.to_string())),
            other => Ok(SpanFieldRef::MapAttr(other.to_string())),
        }
    }

    /// Resolve one bare field name for faceting (mirrors `FieldResolver::resolve_field_name`).
    /// `status`/`kind`/`duration` are not directly facetable columns → error (the UI facets on
    /// `status_text`/`kind_text` / duration buckets separately).
    pub fn resolve_field_name(&self, name: &str) -> Result<SpanFieldRef, ResolveError> {
        match name {
            "status" | "kind" | "duration" => Err(ResolveError {
                message: format!("`{name}` is not a facetable field"),
            }),
            "status_text" => Ok(SpanFieldRef::Attr("status_text".into())),
            "kind_text" => Ok(SpanFieldRef::Attr("kind_text".into())),
            _ => self.resolve_field(name),
        }
    }
}

/// OTLP `StatusCode` keyword → code. Unknown keyword → error.
fn status_codes(values: &[String]) -> Result<Vec<i32>, ResolveError> {
    values
        .iter()
        .map(|v| match v.as_str() {
            "unset" => Ok(0),
            "ok" => Ok(1),
            "error" => Ok(2),
            other => Err(ResolveError {
                message: format!("unknown status `{other}` (use unset|ok|error)"),
            }),
        })
        .collect()
}

/// OTLP `SpanKind` keyword → code. Unknown keyword → error.
fn kind_codes(values: &[String]) -> Result<Vec<i32>, ResolveError> {
    values
        .iter()
        .map(|v| match v.as_str() {
            "unspecified" => Ok(0),
            "internal" => Ok(1),
            "server" => Ok(2),
            "client" => Ok(3),
            "producer" => Ok(4),
            "consumer" => Ok(5),
            other => Err(ResolveError {
                message: format!(
                    "unknown kind `{other}` (use internal|server|client|producer|consumer)"
                ),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parse;

    fn resolver() -> SpanFieldResolver {
        SpanFieldResolver::new(&["service.name".into(), "http.status_code".into()])
    }
    fn r(input: &str) -> SpanResolvedQuery {
        resolver().resolve(&parse(input).unwrap()).unwrap()
    }

    #[test]
    fn aliases_status_kind_duration_and_attrs() {
        assert_eq!(
            r("service:checkout").terms[0].kind,
            SpanResolvedKind::Match {
                field: SpanFieldRef::Attr("service.name".into()),
                values: vec!["checkout".into()]
            }
        );
        assert_eq!(
            r("operation:charge").terms[0].kind,
            SpanResolvedKind::Match {
                field: SpanFieldRef::Name,
                values: vec!["charge".into()]
            }
        );
        assert_eq!(
            r("name:charge").terms[0].kind,
            SpanResolvedKind::Match {
                field: SpanFieldRef::Name,
                values: vec!["charge".into()]
            }
        );
        assert_eq!(
            r("status:error,ok").terms[0].kind,
            SpanResolvedKind::Status { codes: vec![2, 1] }
        );
        assert_eq!(
            r("kind:client").terms[0].kind,
            SpanResolvedKind::Kind { codes: vec![3] }
        );
        assert_eq!(
            r("duration>=500ms").terms[0].kind,
            SpanResolvedKind::Compare {
                field: SpanFieldRef::Duration,
                op: Cmp::Ge,
                value: 500_000_000.0
            }
        );
        assert_eq!(
            r("http.status_code:504").terms[0].kind,
            SpanResolvedKind::Match {
                field: SpanFieldRef::Attr("http.status_code".into()),
                values: vec!["504".into()]
            }
        );
        assert_eq!(
            r("region:us").terms[0].kind,
            SpanResolvedKind::Match {
                field: SpanFieldRef::MapAttr("region".into()),
                values: vec!["us".into()]
            }
        );
        assert!(r("-status:error").terms[0].negated);
    }

    #[test]
    fn rejects_bad_forms() {
        assert!(resolver().resolve(&parse("status:nope").unwrap()).is_err());
        assert!(resolver().resolve(&parse("kind:teapot").unwrap()).is_err());
        assert!(resolver().resolve(&parse("status>=2").unwrap()).is_err());
        assert!(resolver().resolve(&parse("duration:*").unwrap()).is_err());
        assert!(resolver().resolve(&parse("kind:*").unwrap()).is_err());
    }
}
