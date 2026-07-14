//! In-memory evaluation of a `SpanResolvedQuery` against a `SpanRecord`. Semantics MUST mirror
//! `photon_query::span_predicate` exactly (asserted by the two-compiler consistency test): each
//! term computes a positive `base` boolean where an absent/null field yields `false`, then
//! negation flips it (`base ^ negated`).

use super::ast::Cmp;
use super::span_resolver::{SpanFieldRef, SpanResolvedKind, SpanResolvedQuery, SpanResolvedTerm};
use crate::span_record::SpanRecord;

impl SpanResolvedQuery {
    /// True iff `record` satisfies every term (empty query → true).
    pub fn matches(&self, record: &SpanRecord) -> bool {
        self.terms.iter().all(|t| t.matches(record))
    }

    /// Operation-name search texts of positive (non-negated) `FreeText` terms — the only terms that
    /// may drive `name`-bloom pruning. A `-word` term must NOT skip files lacking the word.
    pub fn positive_freetext(&self) -> Vec<&str> {
        self.terms
            .iter()
            .filter_map(|t| match (t.negated, &t.kind) {
                (false, SpanResolvedKind::FreeText { text }) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

impl SpanResolvedTerm {
    fn matches(&self, r: &SpanRecord) -> bool {
        let base = match &self.kind {
            SpanResolvedKind::Status { codes } => r.status_code.is_some_and(|c| codes.contains(&c)),
            SpanResolvedKind::Kind { codes } => r.kind.is_some_and(|c| codes.contains(&c)),
            SpanResolvedKind::Match { field, values } => {
                field_value(field, r).is_some_and(|v| values.iter().any(|x| x == v))
            }
            SpanResolvedKind::Exists { field } => field_value(field, r).is_some(),
            SpanResolvedKind::Compare { field, op, value } => match field {
                // duration_nanos is a real i64 — compare numerically without a string round-trip.
                SpanFieldRef::Duration => {
                    r.duration_nanos.is_some_and(|d| cmp(*op, d as f64, *value))
                }
                _ => field_value(field, r)
                    .and_then(|v| v.parse::<f64>().ok())
                    .is_some_and(|v| cmp(*op, v, *value)),
            },
            SpanResolvedKind::FreeText { text } => {
                r.name.as_deref().is_some_and(|n| n.contains(text.as_str()))
            }
        };
        base ^ self.negated
    }
}

fn field_value<'a>(field: &SpanFieldRef, r: &'a SpanRecord) -> Option<&'a str> {
    match field {
        SpanFieldRef::TraceId => Some(r.trace_id.as_str()),
        SpanFieldRef::SpanId => Some(r.span_id.as_str()),
        SpanFieldRef::ParentSpanId => r.parent_span_id.as_deref(),
        SpanFieldRef::Name => r.name.as_deref(),
        SpanFieldRef::ScopeName => r.scope_name.as_deref(),
        SpanFieldRef::Duration => None, // numeric-only; handled in the Compare arm
        SpanFieldRef::Attr(name) => r.attributes.get(name).map(|s| s.as_str()),
        SpanFieldRef::MapAttr(name) => r.attributes.get(name).map(|s| s.as_str()),
    }
}

fn cmp(op: Cmp, a: f64, b: f64) -> bool {
    match op {
        Cmp::Gt => a > b,
        Cmp::Ge => a >= b,
        Cmp::Lt => a < b,
        Cmp::Le => a <= b,
    }
}

#[cfg(test)]
mod tests {
    use crate::query::{parse, SpanFieldResolver, SpanResolvedQuery};
    use crate::span_record::SpanRecord;
    use std::collections::BTreeMap;

    fn rq(input: &str) -> SpanResolvedQuery {
        SpanFieldResolver::new(&["service.name".into(), "http.status_code".into()])
            .resolve(&parse(input).unwrap())
            .unwrap()
    }

    fn span(
        service: &str,
        name: &str,
        dur: Option<i64>,
        status: Option<i32>,
        kind: Option<i32>,
        attrs: &[(&str, &str)],
    ) -> SpanRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        SpanRecord {
            trace_id: "t1".into(),
            span_id: "s1".into(),
            name: Some(name.into()),
            duration_nanos: dur,
            status_code: status,
            kind,
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn match_status_kind_duration_freetext_and_negation() {
        let s = span(
            "checkout",
            "charge.card",
            Some(600_000_000),
            Some(2),
            Some(3),
            &[("http.status_code", "504")],
        );
        assert!(rq("service:checkout").matches(&s));
        assert!(rq("status:error").matches(&s));
        assert!(!rq("status:ok").matches(&s));
        assert!(rq("kind:client").matches(&s));
        assert!(rq("duration>=500ms").matches(&s));
        assert!(!rq("duration>=1s").matches(&s));
        assert!(rq("charge").matches(&s)); // free-text on name
        assert!(!rq("refund").matches(&s));
        assert!(rq("http.status_code:504").matches(&s));
        assert!(rq("-status:ok").matches(&s));
        assert!(rq("service:checkout status:error duration>=500ms").matches(&s));
    }

    #[test]
    fn absent_and_null_fields() {
        let s = span("api", "op", None, None, None, &[]);
        assert!(!rq("status:error").matches(&s));
        assert!(rq("-status:error").matches(&s));
        assert!(!rq("duration>=1ms").matches(&s));
        assert!(rq("-duration>=1ms").matches(&s));
        assert!(!rq("http.status_code:504").matches(&s));
        assert!(rq("-http.status_code:504").matches(&s));
    }
}
