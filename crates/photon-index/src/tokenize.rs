//! Shared tokenizer. The SAME function is used both when building the index (over the
//! `body` column) and at query time (over the user's search string) — `photon-compact`
//! and `photon-query` both import this. If the two diverged, pruning would drop real
//! results, so keep this simple and stable.

use std::collections::HashSet;

/// Split already-lowercased `lowered` on any non-alphanumeric char, dropping empty tokens.
/// Shared by `tokenize` and `tokenize_dedup_into` so the two definitions can't diverge.
fn split_tokens(lowered: &str) -> impl Iterator<Item = &str> {
    lowered
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
}

/// Lowercase `text`, split on any non-alphanumeric char, and drop empty tokens.
///
/// Milestone-1 definition (n-gram tokens are a later, off-by-default addition).
pub fn tokenize(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    split_tokens(&lowered).map(str::to_string).collect()
}

/// Like `tokenize`, but inserts distinct tokens directly into `out` instead of returning a
/// `Vec` with one allocation per occurrence. A `String` is only allocated for a token the
/// first time it's seen — log bodies repeat tokens heavily, so this keeps allocation count
/// (and downstream bloom-insert hashing) at O(distinct tokens) rather than O(total tokens).
pub(crate) fn tokenize_dedup_into(text: &str, out: &mut HashSet<String>) {
    let lowered = text.to_lowercase();
    for token in split_tokens(&lowered) {
        if !out.contains(token) {
            out.insert(token.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_splits_on_punctuation() {
        assert_eq!(tokenize("Hello, World!"), vec!["hello", "world"]);
    }

    #[test]
    fn collapses_runs_of_punctuation_and_drops_empty_tokens() {
        assert_eq!(tokenize("  --foo__bar--  "), vec!["foo", "bar"]);
    }

    #[test]
    fn empty_and_all_punctuation_strings_yield_no_tokens() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("!!! --- ???").is_empty());
    }

    #[test]
    fn digits_and_mixed_case_are_preserved_within_a_token() {
        assert_eq!(tokenize("Error404_NotFound"), vec!["error404", "notfound"]);
    }

    #[test]
    fn punctuation_heavy_sentence_splits_into_expected_words() {
        assert_eq!(
            tokenize("connection.timeout: retrying (attempt #3)"),
            vec!["connection", "timeout", "retrying", "attempt", "3"]
        );
    }

    #[test]
    fn dedup_into_matches_tokenize_as_a_distinct_set() {
        let text = "retry retry retry connection Timeout timeout";
        let mut set = HashSet::new();
        tokenize_dedup_into(text, &mut set);

        let expected: HashSet<String> = tokenize(text).into_iter().collect();
        assert_eq!(set, expected);
        assert_eq!(set.len(), 3); // retry, connection, timeout
    }

    #[test]
    fn dedup_into_accumulates_across_multiple_calls() {
        let mut set = HashSet::new();
        tokenize_dedup_into("alpha beta", &mut set);
        tokenize_dedup_into("beta gamma", &mut set);
        let mut sorted: Vec<&String> = set.iter().collect();
        sorted.sort();
        assert_eq!(sorted, vec!["alpha", "beta", "gamma"]);
    }
}
