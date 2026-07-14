//! Bearer-token check shared by the gRPC and HTTP receivers.
//!
//! Factored out as a pure function so the auth logic is unit-testable without spinning up
//! a live server (gRPC metadata and HTTP headers both reduce to `Option<&str>` + the
//! expected token).

use subtle::ConstantTimeEq;

/// Returns `true` iff `header_value` is exactly `Bearer <token>`.
///
/// The token equality check is constant-time (`subtle::ConstantTimeEq`) so a byte-by-byte
/// `==` scan can't leak a timing side-channel on the shared ingest secret. Everything else
/// (missing header, missing/incorrect `Bearer ` scheme) is not secret-dependent and stays a
/// plain, short-circuiting comparison.
pub(crate) fn check_bearer_token(header_value: Option<&str>, token: &str) -> bool {
    match header_value {
        Some(v) => v
            .strip_prefix("Bearer ")
            .is_some_and(|t| bool::from(t.as_bytes().ct_eq(token.as_bytes()))),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_bearer_token_passes() {
        assert!(check_bearer_token(Some("Bearer good"), "good"));
    }

    #[test]
    fn wrong_token_fails() {
        assert!(!check_bearer_token(Some("Bearer bad"), "good"));
    }

    #[test]
    fn missing_header_fails() {
        assert!(!check_bearer_token(None, "good"));
    }

    #[test]
    fn missing_bearer_prefix_fails() {
        assert!(!check_bearer_token(Some("good"), "good"));
    }

    #[test]
    fn case_sensitive_scheme_fails() {
        assert!(!check_bearer_token(Some("bearer good"), "good"));
    }

    #[test]
    fn empty_expected_token_still_requires_prefix() {
        assert!(check_bearer_token(Some("Bearer "), ""));
        assert!(!check_bearer_token(Some(""), ""));
    }
}
