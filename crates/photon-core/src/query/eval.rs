//! In-memory evaluation of a `ResolvedQuery` against a `LogRecord`. Semantics must mirror
//! `photon_query::predicate` exactly (asserted by `photon-query`'s consistency test): each
//! term computes a positive `base` boolean where an absent/null field yields `false`, then
//! negation flips it (`base ^ negated`).

use super::ast::Cmp;
use super::resolver::{FieldRef, ResolvedKind, ResolvedQuery, ResolvedTerm};
use crate::record::LogRecord;

impl ResolvedQuery {
    /// True iff `record` satisfies every term (empty query → true).
    pub fn matches(&self, record: &LogRecord) -> bool {
        self.terms.iter().all(|t| t.matches(record))
    }

    /// Body-search texts of positive (non-negated) `FreeText` terms — the only terms that
    /// may drive bloom pruning. A `-word` term must NOT skip files lacking the word.
    pub fn positive_freetext(&self) -> Vec<&str> {
        self.terms
            .iter()
            .filter_map(|t| match (t.negated, &t.kind) {
                (false, ResolvedKind::FreeText { text }) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

impl ResolvedTerm {
    fn matches(&self, r: &LogRecord) -> bool {
        let base = match &self.kind {
            ResolvedKind::Level { ranges } => r
                .severity_number
                .is_some_and(|n| ranges.iter().any(|(lo, hi)| *lo <= n && n <= *hi)),
            ResolvedKind::Match { field, values } => {
                field_value(field, r).is_some_and(|v| values.iter().any(|x| x == v))
            }
            ResolvedKind::Exists { field } => field_value(field, r).is_some(),
            ResolvedKind::Compare { field, op, value } => field_value(field, r)
                .and_then(|v| v.parse::<f64>().ok())
                .is_some_and(|v| cmp(*op, v, *value)),
            ResolvedKind::FreeText { text } => {
                r.body.as_deref().is_some_and(|b| b.contains(text.as_str()))
            }
        };
        base ^ self.negated
    }
}

fn field_value<'a>(field: &FieldRef, r: &'a LogRecord) -> Option<&'a str> {
    match field {
        FieldRef::TraceId => r.trace_id.as_deref(),
        FieldRef::SpanId => r.span_id.as_deref(),
        FieldRef::SeverityText => r.severity_text.as_deref(),
        FieldRef::Attr(name) => r.attributes.get(name).map(|s| s.as_str()),
        FieldRef::MapAttr(name) => r.attributes.get(name).map(|s| s.as_str()),
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
    use crate::query::{parse, FieldResolver};
    use crate::record::LogRecord;
    use std::collections::BTreeMap;

    fn resolver() -> FieldResolver {
        FieldResolver::new(&[
            "service.name".into(),
            "host.name".into(),
            "status_code".into(),
        ])
    }

    fn rq(input: &str) -> crate::query::ResolvedQuery {
        resolver().resolve(&parse(input).unwrap()).unwrap()
    }

    fn rec(service: &str, sev: Option<i32>, body: &str, attrs: &[(&str, &str)]) -> LogRecord {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".into(), service.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        LogRecord {
            timestamp_nanos: 1,
            severity_number: sev,
            body: Some(body.into()),
            attributes,
            ..Default::default()
        }
    }

    #[test]
    fn match_exists_compare_level_freetext_and_negation() {
        let r = rec(
            "api",
            Some(18),
            "connection timeout",
            &[("host.name", "api-1"), ("status_code", "500")],
        );
        assert!(rq("service:api").matches(&r));
        assert!(!rq("service:web").matches(&r));
        assert!(rq("host.name:*").matches(&r));
        assert!(rq("status_code>=500").matches(&r));
        assert!(!rq("status_code<500").matches(&r));
        assert!(rq("level:error").matches(&r)); // 18 -> error
        assert!(rq("timeout").matches(&r));
        assert!(rq("-service:web").matches(&r));
        assert!(rq("service:api status_code>=500 \"timeout\"").matches(&r));
    }

    #[test]
    fn absent_and_null_fields() {
        let r = rec("api", None, "ok", &[]); // no host.name, no status_code, null severity
        assert!(!rq("host.name:*").matches(&r));
        assert!(rq("-host.name:*").matches(&r)); // absent → negated-exists true
        assert!(!rq("status_code>=500").matches(&r)); // absent → compare false
        assert!(rq("-status_code>=500").matches(&r));
        assert!(!rq("level:error").matches(&r)); // null severity → false
        assert!(rq("-level:error").matches(&r));
    }

    #[test]
    fn positive_freetext_excludes_negated() {
        let q = rq("foo -bar \"baz qux\"");
        assert_eq!(q.positive_freetext(), vec!["foo", "baz qux"]);
    }
}
