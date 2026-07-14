//! Smart default aggregation — the DX keystone. A pure function of a metric's `metric_type`
//! and (for Sums) `is_monotonic`, so a single metric charts correctly with zero configuration:
//! monotonic Sum → rate, Histogram/Exp-histogram → p99, Gauge → avg, Summary → its median
//! quantile, non-monotonic Sum → sum. Computed server-side and echoed to the UI.

use crate::metric_schema::metric_type;

/// The aggregation applied to a metric's points over each time bucket. `P50/P90/P99` (histogram
/// quantiles) and `Median` (summary) are defined here so `default_agg` is total, but the Phase-2
/// query engine only *evaluates* the number aggregations (`Rate`..`Count`); it returns a clear
/// error for the quantile aggregations, which Phases 4–5 implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agg {
    Rate,
    Increase,
    Sum,
    Avg,
    Min,
    Max,
    Last,
    Count,
    P50,
    P90,
    P99,
    Median,
}

impl Agg {
    pub fn as_str(self) -> &'static str {
        match self {
            Agg::Rate => "rate",
            Agg::Increase => "increase",
            Agg::Sum => "sum",
            Agg::Avg => "avg",
            Agg::Min => "min",
            Agg::Max => "max",
            Agg::Last => "last",
            Agg::Count => "count",
            Agg::P50 => "p50",
            Agg::P90 => "p90",
            Agg::P99 => "p99",
            Agg::Median => "median",
        }
    }

    pub fn parse(s: &str) -> Option<Agg> {
        Some(match s {
            "rate" => Agg::Rate,
            "increase" => Agg::Increase,
            "sum" => Agg::Sum,
            "avg" => Agg::Avg,
            "min" => Agg::Min,
            "max" => Agg::Max,
            "last" => Agg::Last,
            "count" => Agg::Count,
            "p50" => Agg::P50,
            "p90" => Agg::P90,
            "p99" => Agg::P99,
            "median" => Agg::Median,
            _ => return None,
        })
    }
}

/// The aggregation Photon picks automatically for a metric that carries no explicit `agg`.
pub fn default_agg(metric_type: i32, is_monotonic: Option<bool>) -> Agg {
    match metric_type {
        metric_type::GAUGE => Agg::Avg,
        metric_type::SUM => {
            if is_monotonic == Some(true) {
                Agg::Rate
            } else {
                Agg::Sum
            }
        }
        metric_type::HISTOGRAM | metric_type::EXP_HISTOGRAM => Agg::P99,
        metric_type::SUMMARY => Agg::Median,
        _ => Agg::Avg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metric_schema::metric_type;

    #[test]
    fn default_agg_matches_spec() {
        assert_eq!(default_agg(metric_type::GAUGE, None), Agg::Avg);
        assert_eq!(default_agg(metric_type::SUM, Some(true)), Agg::Rate);
        assert_eq!(default_agg(metric_type::SUM, Some(false)), Agg::Sum);
        assert_eq!(default_agg(metric_type::SUM, None), Agg::Sum); // unknown monotonicity → sum
        assert_eq!(default_agg(metric_type::HISTOGRAM, None), Agg::P99);
        assert_eq!(default_agg(metric_type::EXP_HISTOGRAM, None), Agg::P99);
        assert_eq!(default_agg(metric_type::SUMMARY, None), Agg::Median);
        assert_eq!(default_agg(999, None), Agg::Avg); // unknown type → safe fallback
    }

    #[test]
    fn agg_str_roundtrips() {
        for a in [
            Agg::Rate,
            Agg::Increase,
            Agg::Sum,
            Agg::Avg,
            Agg::Min,
            Agg::Max,
            Agg::Last,
            Agg::Count,
            Agg::P50,
            Agg::P90,
            Agg::P99,
            Agg::Median,
        ] {
            assert_eq!(Agg::parse(a.as_str()), Some(a), "roundtrip {a:?}");
        }
        assert_eq!(Agg::parse("p95"), None);
        assert_eq!(Agg::parse(""), None);
    }
}
