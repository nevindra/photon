//! Turn an errored span into a **Backend Issue**: `error.type`, `error.message`,
//! `error.fingerprint`, written as ordinary span attributes at ingest.
//!
//! `CONTEXT.md` has defined Backend Issue for a while with nothing producing one. This is the
//! producer. It reuses `rum::fingerprint` verbatim rather than growing a second grouping rule —
//! a Frontend Issue and a Backend Issue are the same idea about two different runtimes, and two
//! hash functions for one concept is how "the same error" stops meaning the same thing.
//!
//! **Fingerprinting happens at INGEST, not at query time.** A fingerprint is a property of the
//! error, so it is computed once when the error arrives and stored beside it. The cost of that
//! choice is stated plainly and not worked around: spans ingested before this shipped carry no
//! fingerprint and never will. There is deliberately no backfill and no query-time fallback —
//! two grouping rules for one concept is worse than one honest gap in the history.
//!
//! **Cost control.** `classify` returns immediately unless the span already looks like an error
//! (`status_code == 2`, or an event named `exception`). A healthy service pays one integer
//! comparison per span and nothing else.

use crate::rum::fingerprint;

/// The three attribute keys written onto an errored span. Public because both the ingest mappers
/// and the query layer name them, and a typo in one of two string literals is invisible.
pub const ERROR_TYPE: &str = "error.type";
pub const ERROR_MESSAGE: &str = "error.message";
pub const ERROR_FINGERPRINT: &str = "error.fingerprint";

/// The OTel semantic-convention event and attribute names this reads.
pub const EXCEPTION_EVENT: &str = "exception";
pub const EXCEPTION_TYPE: &str = "exception.type";
pub const EXCEPTION_MESSAGE: &str = "exception.message";
pub const EXCEPTION_STACKTRACE: &str = "exception.stacktrace";

/// OTLP `StatusCode::Error`.
pub const STATUS_ERROR: i32 = 2;

/// What gets written onto the span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanError {
    pub error_type: String,
    pub message: String,
    pub fingerprint: String,
}

impl SpanError {
    /// The three attributes, ready to insert into a span's attribute map.
    pub fn attributes(self) -> [(&'static str, String); 3] {
        [
            (ERROR_TYPE, self.error_type),
            (ERROR_MESSAGE, self.message),
            (ERROR_FINGERPRINT, self.fingerprint),
        ]
    }
}

/// One exception event's parts, as the caller reads them out of its own event representation.
///
/// Taking the parts rather than the events themselves keeps this crate free of any OTLP or JSON
/// dependency: the gRPC mapper reads them off `Vec<Event>` before serialization, and a JSON caller
/// reads them off the stored `events` string. Neither pays for the other's format.
#[derive(Debug, Clone, Copy, Default)]
pub struct Exception<'a> {
    pub kind: Option<&'a str>,
    pub message: Option<&'a str>,
    pub stacktrace: Option<&'a str>,
}

/// The first line of a stack trace, trimmed — the frame the fingerprint keys on.
///
/// Only the FIRST frame, deliberately: two occurrences of one bug reached through different call
/// paths share a top frame and differ further down, so hashing the whole trace would split one
/// issue into as many issues as there are ways to reach it.
fn top_frame(stacktrace: Option<&str>) -> Option<&str> {
    stacktrace
        .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
}

/// Classify a span. `None` means "this is not an error" — and that is written as *no attributes*,
/// never as an empty fingerprint, because an empty fingerprint would collect every healthy span in
/// the fleet into one enormous phantom Issue.
///
/// Precedence: a real exception event wins over the span's status, because it carries a type and a
/// frame and the status carries neither. With no exception event, the status message becomes the
/// message and the span's own name becomes the frame — so a service that only sets
/// `status = ERROR` still groups by operation instead of collapsing into one bucket.
pub fn classify(
    status_code: Option<i32>,
    status_message: Option<&str>,
    span_name: Option<&str>,
    exception: Option<Exception<'_>>,
) -> Option<SpanError> {
    let is_error = status_code == Some(STATUS_ERROR);
    let exception = exception.filter(|e| {
        // An exception event with nothing in it says no more than the status does.
        e.kind.is_some() || e.message.is_some() || e.stacktrace.is_some()
    });
    if !is_error && exception.is_none() {
        return None;
    }

    match exception {
        Some(e) => {
            // An exception with no type still has a message and a frame; `exception` is the
            // honest label for it, and it is what OTel calls the event itself.
            let kind = e.kind.filter(|k| !k.is_empty()).unwrap_or(EXCEPTION_EVENT);
            let message = e.message.unwrap_or_default();
            let frame = top_frame(e.stacktrace).or(span_name);
            Some(SpanError {
                error_type: kind.to_string(),
                message: message.to_string(),
                fingerprint: fingerprint(kind, message, frame),
            })
        }
        None => {
            // Status-only error. `ERROR` is the OTLP status text, so the Issue reads
            // "ERROR: gateway timeout" — true, and no more specific than what the span said.
            let message = status_message.unwrap_or_default();
            Some(SpanError {
                error_type: "ERROR".to_string(),
                message: message.to_string(),
                fingerprint: fingerprint("ERROR", message, span_name),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exc<'a>(kind: &'a str, message: &'a str, stack: Option<&'a str>) -> Exception<'a> {
        Exception {
            kind: Some(kind),
            message: Some(message),
            stacktrace: stack,
        }
    }

    #[test]
    fn an_exception_event_produces_a_stable_fingerprint() {
        let a = classify(
            Some(2),
            None,
            Some("GET /a"),
            Some(exc("IOError", "boom", None)),
        )
        .unwrap();
        let b = classify(
            Some(2),
            None,
            Some("GET /a"),
            Some(exc("IOError", "boom", None)),
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.error_type, "IOError");
        assert_eq!(a.fingerprint.len(), 16);
    }

    #[test]
    fn digit_runs_are_masked_so_one_bug_is_one_issue_not_thousands() {
        // "chunk 12 failed" and "chunk 4891 failed" are one bug. Without masking, a retry loop
        // manufactures a new Issue per attempt and the list becomes unusable within an hour.
        let a = classify(Some(2), None, None, Some(exc("E", "chunk 12 failed", None))).unwrap();
        let b = classify(
            Some(2),
            None,
            None,
            Some(exc("E", "chunk 4891 failed", None)),
        )
        .unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(
            a.message, "chunk 12 failed",
            "the RAW message is kept for display"
        );
    }

    #[test]
    fn a_different_exception_type_is_a_different_issue() {
        let a = classify(Some(2), None, None, Some(exc("IOError", "boom", None))).unwrap();
        let b = classify(Some(2), None, None, Some(exc("ValueError", "boom", None))).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn the_same_exception_in_two_services_hashes_the_same() {
        // Deliberately service-FREE. Issue identity is `(service, fingerprint)` at query time, so
        // these are still two Issues with two owners — but the shared hash is what lets a reader
        // see that one bug is present in three services rather than three unrelated bugs.
        let a = classify(Some(2), None, Some("GET /a"), Some(exc("E", "boom", None))).unwrap();
        let b = classify(Some(2), None, Some("GET /a"), Some(exc("E", "boom", None))).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn only_the_top_frame_keys_the_fingerprint() {
        // One bug reached by two call paths shares a top frame and differs further down. Hashing
        // the whole trace would split it into as many Issues as there are ways to reach it.
        let a = classify(
            Some(2),
            None,
            None,
            Some(exc("E", "boom", Some("at db.query\nat handlers.checkout"))),
        )
        .unwrap();
        let b = classify(
            Some(2),
            None,
            None,
            Some(exc("E", "boom", Some("at db.query\nat handlers.refund"))),
        )
        .unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn a_leading_blank_line_in_the_stack_does_not_become_the_frame() {
        let a = classify(
            Some(2),
            None,
            None,
            Some(exc("E", "b", Some("\n  at db.query\n"))),
        )
        .unwrap();
        let b = classify(
            Some(2),
            None,
            None,
            Some(exc("E", "b", Some("at db.query"))),
        )
        .unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn the_status_message_is_the_fallback_when_there_is_no_exception_event() {
        let e = classify(
            Some(2),
            Some("gateway timeout"),
            Some("GET /checkout"),
            None,
        )
        .unwrap();
        assert_eq!(e.error_type, "ERROR");
        assert_eq!(e.message, "gateway timeout");
    }

    #[test]
    fn a_status_only_error_still_groups_by_operation() {
        // Falling back to the span NAME as the frame is what stops every unexplained error in a
        // service collapsing into one useless bucket.
        let a = classify(Some(2), None, Some("GET /a"), None).unwrap();
        let b = classify(Some(2), None, Some("GET /b"), None).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn a_healthy_span_gets_no_error_attributes_at_all() {
        // Not an empty fingerprint: that would collect every healthy span in the fleet into one
        // enormous phantom Issue.
        assert!(classify(Some(1), None, Some("GET /a"), None).is_none());
        assert!(classify(Some(0), None, Some("GET /a"), None).is_none());
        assert!(classify(None, None, Some("GET /a"), None).is_none());
    }

    #[test]
    fn an_empty_exception_event_is_treated_as_no_event() {
        // It says no more than the status does, and letting it through would produce an Issue
        // named after the event rather than the error.
        assert!(classify(None, None, Some("GET /a"), Some(Exception::default())).is_none());
        let e = classify(
            Some(2),
            Some("boom"),
            Some("GET /a"),
            Some(Exception::default()),
        )
        .unwrap();
        assert_eq!(e.error_type, "ERROR", "falls back to the status path");
    }

    #[test]
    fn an_exception_event_on_an_unset_status_still_counts() {
        // Some SDKs record the exception and never set the status. The exception is the error.
        let e = classify(
            None,
            None,
            Some("GET /a"),
            Some(exc("IOError", "boom", None)),
        )
        .unwrap();
        assert_eq!(e.error_type, "IOError");
    }

    #[test]
    fn an_exception_without_a_type_is_labelled_rather_than_left_blank() {
        let e = classify(
            Some(2),
            None,
            None,
            Some(Exception {
                kind: None,
                message: Some("boom"),
                stacktrace: None,
            }),
        )
        .unwrap();
        assert_eq!(e.error_type, EXCEPTION_EVENT);
    }

    #[test]
    fn the_attributes_come_out_under_the_documented_keys() {
        let attrs = classify(Some(2), Some("boom"), Some("GET /a"), None)
            .unwrap()
            .attributes();
        let keys: Vec<&str> = attrs.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![ERROR_TYPE, ERROR_MESSAGE, ERROR_FINGERPRINT]);
    }
}
