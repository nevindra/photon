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

/// The subset of `tokenize(text)` that is safe to drive bloom pruning of a **substring** search:
/// the tokens that are *interior* to `text` — bounded by a non-alphanumeric delimiter on BOTH
/// sides *within `text` itself*.
///
/// Free-text search is substring semantics (`strpos(body, text) > 0` / `body.contains(text)`), but
/// the token bloom holds only the *whole* tokens of the body. A query token that touches the start
/// or end of `text` may be a mere fragment of a longer body word (searching `tim` matches a body
/// reading `timeout`, `timeout` matches `timeouts`), so it is NOT guaranteed to appear as a whole
/// token in a matching body — bloom-testing it would false-*negative* and silently drop a real
/// result. Only a token with a delimiter on both sides inside `text` carries those delimiters into
/// every body that contains `text` as a substring, so its occurrence there is necessarily a whole
/// token → safe to prune on. Concretely:
///   - the first token (when `text` starts with an alphanumeric) may continue a longer word to its
///     left in the body → dropped;
///   - the last token (when `text` ends with an alphanumeric) may continue to its right → dropped;
///   - a single-word query has no interior token → returns empty ⇒ the caller keeps every
///     time-window candidate and lets the row predicate confirm. Correctness over pruning power.
///
/// The delimiter rule is exactly `tokenize`'s (split on any non-alphanumeric char) and the tokens
/// are lowercased identically, so "whole token" means the same thing on the query side as on the
/// index-build side, and the result is always a subset of `tokenize(text)`.
pub fn interior_tokens(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut tokens = Vec::new();
    let mut run_start: Option<usize> = None;
    for (i, c) in lowered.char_indices() {
        if c.is_alphanumeric() {
            run_start.get_or_insert(i);
        } else if let Some(start) = run_start.take() {
            // This run ended at a delimiter (`c`), so it has one on its right. It is interior iff
            // it also has a delimiter on its left, i.e. it did not start at byte 0 of the query.
            if start > 0 {
                tokens.push(lowered[start..i].to_string());
            }
        }
    }
    // A run still open here reached the end of `text` with no trailing delimiter → it touches the
    // right edge → not interior → dropped.
    tokens
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
    fn interior_tokens_drops_edge_tokens() {
        // Single word: the one token touches both edges → could be a fragment of a longer body
        // word → not safe to bloom-test.
        assert!(interior_tokens("tim").is_empty());
        assert!(interior_tokens("timeout").is_empty());
        // Two words: `foo` is first (left edge), `bar` is last (right edge) → neither interior.
        assert!(interior_tokens("foo bar").is_empty());
    }

    #[test]
    fn interior_tokens_keeps_both_sides_delimited_tokens() {
        assert_eq!(interior_tokens("a foo b"), vec!["foo"]);
        assert_eq!(interior_tokens("error timeout id:5"), vec!["timeout", "id"]);
        // Leading/trailing punctuation delimits the outer tokens, making them interior.
        assert_eq!(interior_tokens("  --foo__bar--  "), vec!["foo", "bar"]);
        assert_eq!(interior_tokens(" foo "), vec!["foo"]);
    }

    #[test]
    fn interior_tokens_lowercases_and_is_a_subset_of_tokenize() {
        assert_eq!(interior_tokens("x TiMeOut y"), vec!["timeout"]);
        // Every interior token is a real token of the same string — never a fabricated one.
        for text in [
            "",
            "a foo b",
            "  --foo__bar--  ",
            "Error404_NotFound mid end",
        ] {
            let all: HashSet<String> = tokenize(text).into_iter().collect();
            for t in interior_tokens(text) {
                assert!(all.contains(&t), "{t:?} not a token of {text:?}");
            }
        }
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
