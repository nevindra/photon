//! Shared bucket-INDEX arithmetic for time-bucketed aggregations (logs `histogram`, span
//! `histogram`, and the metrics `query_series` SQL + pointwise paths).
//!
//! All four call sites used to compute `bucket = (ts - start) * buckets / span` — multiply
//! *before* divide, in `i64`. With `buckets` clamped to `MAX_BUCKETS` (3000), `span * buckets`
//! overflows `i64::MAX` once the window exceeds roughly 35 days (`i64::MAX / 3000 ≈ 3.07e15 ns`);
//! an all-time window can overflow at any bucket count. The sibling `bucket_start` label
//! functions (`histogram.rs`, `span_histogram.rs`, `metric_query.rs`) already compute in `i128`
//! and never had this bug, so on a wide window bucket ASSIGNMENT (this module, pre-fix) and
//! bucket LABELS (`bucket_start`) silently disagreed.
//!
//! The fix is divide-first: compute a per-bucket `step` (nanoseconds/bucket) once, then
//! `bucket = (ts - start) / step`. `step * buckets` may be slightly less than `span` (integer
//! division floors), so `ts == end` — and, symmetrically, any `ts` past the last exact boundary —
//! must still clamp to the last bucket; every call site already carried that clamp and it is
//! preserved here.
//!
//! One shared `step`/index computation is used by all four sites (three DataFusion `Expr`
//! builders below share [`bucket_index_expr`]; the Rust-scalar form used by `metric_query`'s
//! `bucket_of` is [`bucket_index`]) so the assignment math can never drift back out of sync with
//! itself across files.

use datafusion::prelude::{lit, when, Expr};
use photon_core::PhotonError;

/// Nanoseconds per bucket for `buckets` equal-width buckets spanning `[start, end]`: `span /
/// buckets`, floored, minimum 1 (so a caller can never divide by zero, matching the pre-existing
/// `span = (end - start).max(1)` guards at every call site).
pub(crate) fn bucket_step(start: i64, end: i64, buckets: usize) -> i64 {
    let span = (end - start).max(1);
    (span / buckets as i64).max(1)
}

/// The bucket-index `Expr` for an already-`Int64` timestamp-like column `ts_col`: divide-first
/// `(ts_col - start) / step`, clamped so `ts_col == end` (or anything past the last exact
/// boundary) lands in the last bucket `buckets - 1`. Shared by `histogram::histogram_over`,
/// `span_histogram::histogram_over`, and `metric_query::bucket_index_expr` — the three sites that
/// bucket rows via a DataFusion aggregate.
pub(crate) fn bucket_index_expr(
    ts_col: Expr,
    start: i64,
    end: i64,
    buckets: usize,
) -> Result<Expr, PhotonError> {
    let step = bucket_step(start, end, buckets);
    let raw = (ts_col - lit(start)) / lit(step);
    when(
        raw.clone().gt_eq(lit(buckets as i64)),
        lit(buckets as i64 - 1),
    )
    .otherwise(raw)
    .map_err(|e| PhotonError::Query(format!("bucket index case: {e}")))
}

/// The bucket index for a raw Rust timestamp: the same divide-first math as
/// [`bucket_index_expr`], in plain `i64`/`usize` arithmetic. Used by `metric_query::bucket_of`,
/// which `reset_aware_series`/`last_series`/the distribution roll-ups in `metric_dist.rs` and the
/// classic-histogram roll-ups in `metric_classic_hist.rs` all walk per-row.
pub(crate) fn bucket_index(ts: i64, start: i64, end: i64, buckets: usize) -> usize {
    let step = bucket_step(start, end, buckets);
    (((ts - start) / step).clamp(0, buckets as i64 - 1)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metric_query::bucket_start;

    const NS_PER_DAY: i64 = 24 * 3600 * 1_000_000_000;

    /// RED (pre-fix) proof: the old multiply-first formula `(end - start) * buckets` overflows
    /// `i64` at a 90-day window with `buckets = 3000` — the exact scenario the audit flagged.
    /// `checked_mul` returning `None` is the overflow the old code hit (a debug-build panic; a
    /// silent wraparound in release). The divide-first fix in this module never performs that
    /// multiplication at all.
    #[test]
    fn old_multiply_first_formula_overflows_at_90_days() {
        let start = 0i64;
        let end = 90 * NS_PER_DAY;
        let buckets = 3000i64;
        assert!(
            (end - start).checked_mul(buckets).is_none(),
            "expected the naive multiply-first math to overflow i64 at this window width"
        );
    }

    /// GREEN: divide-first stays in range and produces sane, monotonic results across the exact
    /// window width that overflows the old formula.
    #[test]
    fn wide_window_bucket_index_is_finite_and_in_range() {
        let start = 0i64;
        let end = 90 * NS_PER_DAY;
        let buckets = 3000usize;

        assert_eq!(bucket_index(start, start, end, buckets), 0);
        assert_eq!(bucket_index(end, start, end, buckets), buckets - 1);
        // Midpoint lands close to the middle bucket; integer-division flooring of `step` can put
        // it one bucket either side of the exact half, which is fine — it's not the alignment
        // property under test here (see `ninety_day_window_index_matches_bucket_start_labels`).
        let mid = bucket_index(end / 2, start, end, buckets);
        assert!(
            (buckets / 2 - 1..=buckets / 2).contains(&mid),
            "expected the midpoint bucket near {}, got {mid}",
            buckets / 2
        );

        // Even a near-i64::MAX window (an "all-time" view) must not panic or wrap negative.
        let huge_end = i64::MAX / 2;
        let idx = bucket_index(huge_end, 0, huge_end, buckets);
        assert_eq!(idx, buckets - 1);
        assert!(bucket_index(huge_end / 3, 0, huge_end, buckets) < buckets);
    }

    /// THE alignment test the audit asks for: at a 90-day window with `buckets = 3000`, the
    /// bucket assigned by `bucket_index` (this module's divide-first index math) for a range of
    /// timestamps — bucket-start boundaries, one nanosecond after a boundary, one nanosecond
    /// before the next boundary, plus window start/mid/end — always falls inside
    /// `[bucket_start(b), bucket_start(b + 1))` (last bucket's upper bound is inclusive of `end`).
    /// `bucket_start` is `metric_query`'s i128 label function — byte-for-byte identical to the
    /// copies in `histogram.rs` and `span_histogram.rs` — so this also certifies assignment
    /// doesn't drift from the labels those two modules render.
    #[test]
    fn ninety_day_window_index_matches_bucket_start_labels() {
        let start = 0i64;
        let end = 90 * NS_PER_DAY;
        let buckets = 3000usize;

        let check = |ts: i64, b: usize| {
            let idx = bucket_index(ts, start, end, buckets);
            assert_eq!(
                idx,
                b,
                "ts={ts} expected bucket {b} (bucket_start({b})={}..{}) got {idx}",
                bucket_start(start, end, buckets, b),
                if b + 1 < buckets {
                    bucket_start(start, end, buckets, b + 1)
                } else {
                    end + 1
                }
            );
        };

        // Window start / mid / end.
        check(start, 0);
        check(end, buckets - 1);
        check(
            (start + end) / 2,
            bucket_index((start + end) / 2, start, end, buckets),
        );

        // Every bucket boundary: the label's own start, one ns after it, and one ns before the
        // next boundary — all three must resolve to the same bucket index `b`.
        for b in 0..buckets {
            let lo = bucket_start(start, end, buckets, b);
            let hi = if b + 1 < buckets {
                bucket_start(start, end, buckets, b + 1)
            } else {
                end + 1
            };
            check(lo, b);
            if lo + 1 < hi {
                check(lo + 1, b);
            }
            if hi - 1 > lo {
                check(hi - 1, b);
            }
        }
    }

    /// `step * buckets <= span`: the correctness note from the audit brief — integer division
    /// floors, so the computed step never overshoots the window, and the `>= buckets` clamp is
    /// what absorbs the resulting last partial bucket.
    #[test]
    fn step_times_buckets_never_exceeds_span() {
        let start = 0i64;
        let end = 90 * NS_PER_DAY;
        let buckets = 3000usize;
        let step = bucket_step(start, end, buckets);
        assert!(step * buckets as i64 <= end - start);
    }

    /// `ts == end` must clamp to the last bucket whenever `step`'s floor makes
    /// `(end - start) / step` reach or exceed `buckets` — the everyday case, since `step =
    /// floor(span / buckets)` means `span / step >= buckets`. Uses a span that is an exact
    /// multiple of `buckets` so `(end - start) / step == buckets` exactly, hitting the `>=`
    /// branch of the clamp deliberately (not just approximately).
    #[test]
    fn ts_at_end_clamps_to_last_bucket() {
        let (start, buckets) = (0i64, 3000usize);
        let end = 2 * buckets as i64; // step == 2, so (end - start) / step == buckets exactly.
        assert_eq!(bucket_step(start, end, buckets), 2);
        assert_eq!(bucket_index(end, start, end, buckets), buckets - 1);
        assert_eq!(bucket_index(start, start, end, buckets), 0);
    }

    /// A window narrower than `buckets` (span in nanoseconds smaller than the bucket count):
    /// `step` floors to the `.max(1)` guard rather than 0, and every index still stays in
    /// `[0, buckets - 1]` — most of the tail buckets are simply never hit, which is fine (they
    /// keep their `bucket_start` label and a `None`/zero value).
    #[test]
    fn narrow_window_step_floors_to_one_and_stays_in_range() {
        let (start, end, buckets) = (0i64, 10i64, 3000usize);
        assert_eq!(bucket_step(start, end, buckets), 1);
        assert_eq!(bucket_index(start, start, end, buckets), 0);
        let idx = bucket_index(end, start, end, buckets);
        assert_eq!(
            idx, 10,
            "step=1 ⇒ no clamping needed, ts=end just maps to ts itself"
        );
        assert!(idx < buckets);
    }
}
