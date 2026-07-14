//! In-memory evaluation of a resolved metrics query against a `MetricPoint` — the second of the
//! two compilers (the SQL compiler is `photon-query/src/metric_predicate.rs`). Kept
//! result-identical to the SQL compiler by the two-backend consistency test.

use crate::metric_record::MetricPoint;
use crate::query::ast::Cmp;
use crate::query::metric_resolver::{
    MetricFieldRef, MetricResolvedKind, MetricResolvedQuery, MetricResolvedTerm,
};

impl MetricResolvedQuery {
    pub fn matches(&self, p: &MetricPoint) -> bool {
        self.terms.iter().all(|t| t.matches(p))
    }
}

impl MetricResolvedTerm {
    fn matches(&self, p: &MetricPoint) -> bool {
        let base = match &self.kind {
            MetricResolvedKind::Match { field, values } => {
                field_value(field, p).is_some_and(|v| values.iter().any(|x| x == v))
            }
            MetricResolvedKind::Exists { field } => field_value(field, p).is_some(),
            MetricResolvedKind::Compare { field, op, value } => field_value(field, p)
                .and_then(|v| v.parse::<f64>().ok())
                .is_some_and(|v| cmp(*op, v, *value)),
        };
        base ^ self.negated
    }
}

/// Both `Attr` and `MapAttr` read from `point.attributes` in memory — promotion to a dedicated
/// column happens only in the Arrow builder, so on the flat struct every label (including
/// `service.name`) lives in the one attributes map. The `Attr`/`MapAttr` split matters only to
/// the SQL compiler (promoted column vs `attributes[key]`).
fn field_value<'a>(field: &MetricFieldRef, p: &'a MetricPoint) -> Option<&'a str> {
    match field {
        MetricFieldRef::ScopeName => p.scope_name.as_deref(),
        MetricFieldRef::Attr(name) | MetricFieldRef::MapAttr(name) => {
            p.attributes.get(name).map(|s| s.as_str())
        }
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
    use super::*;
    use crate::query::metric_resolver::MetricFieldResolver;
    use crate::query::parser::parse;
    use std::collections::BTreeMap;

    fn point(attrs: &[(&str, &str)], scope: Option<&str>) -> MetricPoint {
        let mut attributes = BTreeMap::new();
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        MetricPoint {
            metric_name: "http.server.duration".to_string(),
            metric_type: 0,
            type_text: None,
            temporality: None,
            is_monotonic: None,
            unit: None,
            timestamp_nanos: 0,
            start_timestamp_nanos: None,
            scope_name: scope.map(|s| s.to_string()),
            value: Some(1.0),
            histogram: None,
            exp_histogram: None,
            summary: None,
            exemplars: None,
            attributes,
        }
    }

    fn matches(q: &str, p: &MetricPoint) -> bool {
        let r = MetricFieldResolver::new(&["service.name".to_string()]);
        r.resolve(&parse(q).unwrap()).unwrap().matches(p)
    }

    #[test]
    fn match_exists_negate_compare_scope() {
        let p = point(
            &[("service.name", "checkout"), ("http.status_code", "503")],
            Some("otel"),
        );
        assert!(matches("service:checkout", &p));
        assert!(!matches("service:cart", &p));
        assert!(matches("service:cart,checkout", &p)); // OR-list
        assert!(matches("http.status_code:*", &p));
        assert!(matches("-service:cart", &p));
        assert!(matches("http.status_code>=500", &p));
        assert!(!matches("http.status_code<500", &p));
        assert!(matches("scope:otel", &p));
        assert!(!matches("region:*", &p)); // absent → Exists false
    }
}
