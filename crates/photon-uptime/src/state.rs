//! The pure up/down state machine. No I/O, exhaustively table-tested.

use crate::model::{CheckResult, MonitorState, RuntimeState, Transition};

/// Fold one check result into the runtime state, emitting a transition worth notifying on.
///
/// `retries` is the number of consecutive failures required before a monitor is declared
/// DOWN (treated as "go down on the Nth failure"; `retries <= 1` ⇒ down on the first).
pub fn apply(
    prev: RuntimeState,
    result: &CheckResult,
    retries: u32,
) -> (RuntimeState, Option<Transition>) {
    let threshold = retries.max(1);
    if result.ok {
        let next = RuntimeState {
            state: MonitorState::Up,
            consecutive_failures: 0,
        };
        let transition = (prev.state == MonitorState::Down).then_some(Transition::Recovered);
        (next, transition)
    } else {
        let failures = prev.consecutive_failures + 1;
        if failures >= threshold && prev.state != MonitorState::Down {
            (
                RuntimeState {
                    state: MonitorState::Down,
                    consecutive_failures: failures,
                },
                Some(Transition::WentDown),
            )
        } else {
            // Below threshold, or already down: record the failure, no transition.
            let state = prev.state;
            (
                RuntimeState {
                    state,
                    consecutive_failures: failures,
                },
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply;
    use crate::model::{CheckResult, MonitorState, RuntimeState, Transition};

    fn ok() -> CheckResult {
        CheckResult {
            ok: true,
            latency_ms: 5,
            status_code: Some(200),
            error: None,
        }
    }
    fn fail() -> CheckResult {
        CheckResult {
            ok: false,
            latency_ms: 0,
            status_code: None,
            error: Some("x".into()),
        }
    }

    #[test]
    fn first_success_from_pending_goes_up_no_transition() {
        let (next, t) = apply(RuntimeState::pending(), &ok(), 3);
        assert_eq!(next.state, MonitorState::Up);
        assert_eq!(next.consecutive_failures, 0);
        assert_eq!(t, None);
    }

    #[test]
    fn stays_up_until_threshold_then_goes_down_once() {
        let mut s = RuntimeState::from_state(MonitorState::Up);
        // retries = 3 → down only on the 3rd consecutive failure
        let (s1, t1) = apply(s, &fail(), 3);
        s = s1;
        assert_eq!(s.state, MonitorState::Up);
        assert_eq!(t1, None);
        let (s2, t2) = apply(s, &fail(), 3);
        s = s2;
        assert_eq!(s.state, MonitorState::Up);
        assert_eq!(t2, None);
        let (s3, t3) = apply(s, &fail(), 3);
        s = s3;
        assert_eq!(s.state, MonitorState::Down);
        assert_eq!(t3, Some(Transition::WentDown));
        // already down → no repeat transition
        let (s4, t4) = apply(s, &fail(), 3);
        assert_eq!(s4.state, MonitorState::Down);
        assert_eq!(t4, None);
    }

    #[test]
    fn recovery_from_down_emits_once_and_resets() {
        let s = RuntimeState {
            state: MonitorState::Down,
            consecutive_failures: 5,
        };
        let (next, t) = apply(s, &ok(), 3);
        assert_eq!(next.state, MonitorState::Up);
        assert_eq!(next.consecutive_failures, 0);
        assert_eq!(t, Some(Transition::Recovered));
    }

    #[test]
    fn intermittent_failure_resets_the_counter() {
        let mut s = RuntimeState::from_state(MonitorState::Up);
        let (s1, _) = apply(s, &fail(), 3);
        s = s1;
        assert_eq!(s.consecutive_failures, 1);
        let (s2, _) = apply(s, &ok(), 3);
        s = s2; // blip recovered before threshold
        assert_eq!(s.consecutive_failures, 0);
        assert_eq!(s.state, MonitorState::Up);
    }

    #[test]
    fn retries_zero_or_one_goes_down_on_first_failure() {
        let (n1, t1) = apply(RuntimeState::from_state(MonitorState::Up), &fail(), 1);
        assert_eq!(n1.state, MonitorState::Down);
        assert_eq!(t1, Some(Transition::WentDown));
        let (n0, t0) = apply(RuntimeState::from_state(MonitorState::Up), &fail(), 0);
        assert_eq!(n0.state, MonitorState::Down);
        assert_eq!(t0, Some(Transition::WentDown));
    }
}
