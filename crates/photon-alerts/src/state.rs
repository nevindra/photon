//! Pure per-(rule,series) lifecycle: Ok → Pending → Triggered. No I/O; exhaustively table-tested.
use crate::model::{AlertPhase, SeriesState, Transition};

/// Advance one (rule, series) state machine by one evaluation tick.
///
/// `for_secs` is **seconds**; `since`/`now` are **ms** — hence `for_secs * 1000` below.
pub fn apply(
    prev: SeriesState,
    breaching: bool,
    value: f64,
    for_secs: i64,
    now: i64,
) -> (SeriesState, Option<Transition>) {
    if breaching {
        match prev.phase {
            AlertPhase::Triggered => (
                SeriesState {
                    phase: AlertPhase::Triggered,
                    since: prev.since,
                    last_value: value,
                },
                None,
            ),
            AlertPhase::Pending => {
                if now - prev.since >= for_secs * 1000 {
                    (
                        SeriesState {
                            phase: AlertPhase::Triggered,
                            since: now,
                            last_value: value,
                        },
                        Some(Transition::Triggered),
                    )
                } else {
                    (
                        SeriesState {
                            phase: AlertPhase::Pending,
                            since: prev.since,
                            last_value: value,
                        },
                        None,
                    )
                }
            }
            AlertPhase::Ok => {
                if for_secs <= 0 {
                    (
                        SeriesState {
                            phase: AlertPhase::Triggered,
                            since: now,
                            last_value: value,
                        },
                        Some(Transition::Triggered),
                    )
                } else {
                    (
                        SeriesState {
                            phase: AlertPhase::Pending,
                            since: now,
                            last_value: value,
                        },
                        None,
                    )
                }
            }
        }
    } else {
        match prev.phase {
            AlertPhase::Triggered => (
                SeriesState {
                    phase: AlertPhase::Ok,
                    since: now,
                    last_value: value,
                },
                Some(Transition::Resolved),
            ),
            _ => (
                SeriesState {
                    phase: AlertPhase::Ok,
                    since: now,
                    last_value: value,
                },
                None,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply;
    use crate::model::{AlertPhase, SeriesState, Transition};

    #[test]
    fn immediate_trigger_when_for_zero() {
        let (s, t) = apply(SeriesState::ok(), true, 9.0, 0, 100);
        assert_eq!(s.phase, AlertPhase::Triggered);
        assert_eq!(t, Some(Transition::Triggered));
    }
    #[test]
    fn pending_then_trigger_after_for_elapses() {
        let (s1, t1) = apply(SeriesState::ok(), true, 9.0, 300, 0);
        assert_eq!(s1.phase, AlertPhase::Pending);
        assert_eq!(t1, None);
        let (s2, t2) = apply(s1, true, 9.5, 300, 200_000); // 200s < 300s
        assert_eq!(s2.phase, AlertPhase::Pending);
        assert_eq!(t2, None);
        let (s3, t3) = apply(s2, true, 9.9, 300, 300_000); // 300s ≥ 300s
        assert_eq!(s3.phase, AlertPhase::Triggered);
        assert_eq!(t3, Some(Transition::Triggered));
    }
    #[test]
    fn no_reemit_while_triggered_and_tracks_last_value() {
        let (s, t) = apply(SeriesState::triggered_since(0), true, 12.0, 0, 500);
        assert_eq!(s.phase, AlertPhase::Triggered);
        assert_eq!(s.last_value, 12.0);
        assert_eq!(t, None);
    }
    #[test]
    fn resolve_from_triggered() {
        let (s, t) = apply(SeriesState::triggered_since(0), false, 1.0, 0, 700);
        assert_eq!(s.phase, AlertPhase::Ok);
        assert_eq!(t, Some(Transition::Resolved));
    }
    #[test]
    fn pending_clears_without_emit() {
        let (s1, _) = apply(SeriesState::ok(), true, 9.0, 300_000, 0);
        let (s2, t2) = apply(s1, false, 1.0, 300_000, 50_000);
        assert_eq!(s2.phase, AlertPhase::Ok);
        assert_eq!(t2, None);
    }
    #[test]
    fn ok_stays_ok_without_emit() {
        let (s, t) = apply(SeriesState::ok(), false, 0.0, 0, 1);
        assert_eq!(s.phase, AlertPhase::Ok);
        assert_eq!(t, None);
    }
}
