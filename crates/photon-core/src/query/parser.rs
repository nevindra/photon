//! Parse a query string into the raw AST. Whitespace separates terms; a double-quoted span
//! is one term even if it contains spaces. Errors carry the byte offset of the bad token.

use super::ast::{Cmp, Query, Term, TermKind};

/// A grammar parse failure with the byte offset (into the original input) of the offending
/// token, so the UI can point at it.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at offset {})", self.message, self.offset)
    }
}

pub fn parse(input: &str) -> Result<Query, ParseError> {
    let mut terms = Vec::new();
    for (offset, token) in tokenize(input)? {
        terms.push(classify(&token, offset)?);
    }
    Ok(Query { terms })
}

/// Split into `(byte_offset, token)` pairs. A `"` opens a quoted span that ends at the next
/// `"`; whitespace outside quotes separates tokens. Tokens keep their quotes / leading `-`.
fn tokenize(input: &str) -> Result<Vec<(usize, String)>, ParseError> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let mut have = false;
    for (i, ch) in input.char_indices() {
        if in_quotes {
            cur.push(ch);
            if ch == '"' {
                in_quotes = false;
            }
        } else if ch == '"' {
            if !have {
                start = i;
                have = true;
            }
            cur.push(ch);
            in_quotes = true;
        } else if ch.is_whitespace() {
            if have {
                out.push((start, std::mem::take(&mut cur)));
                have = false;
            }
        } else {
            if !have {
                start = i;
                have = true;
            }
            cur.push(ch);
        }
    }
    if in_quotes {
        return Err(ParseError {
            message: "unterminated quote".into(),
            offset: start,
        });
    }
    if have {
        out.push((start, cur));
    }
    Ok(out)
}

/// Classify one token. Order: quoted phrase → `field:...` → `field<op>n` → bare word.
/// Colon is checked before comparison operators so a value like `a>b` in `k:a>b` is a value,
/// not a comparison.
fn classify(token: &str, offset: usize) -> Result<Term, ParseError> {
    let (negated, body) = match token.strip_prefix('-') {
        Some(rest) if !rest.is_empty() => (true, rest),
        _ => (false, token),
    };

    if let Some(rest) = body.strip_prefix('"') {
        let text = rest.strip_suffix('"').ok_or(ParseError {
            message: "unterminated quote".into(),
            offset,
        })?;
        return Ok(Term {
            negated,
            kind: TermKind::FreeText {
                text: text.to_string(),
            },
        });
    }

    if let Some((field, rest)) = body.split_once(':') {
        if field.is_empty() {
            return Err(ParseError {
                message: "missing field before ':'".into(),
                offset,
            });
        }
        if rest == "*" {
            return Ok(Term {
                negated,
                kind: TermKind::Exists {
                    field: field.to_string(),
                },
            });
        }
        let values: Vec<String> = rest.split(',').map(|s| s.to_string()).collect();
        if values.iter().any(|v| v.is_empty()) {
            return Err(ParseError {
                message: "empty value in list".into(),
                offset,
            });
        }
        return Ok(Term {
            negated,
            kind: TermKind::Match {
                field: field.to_string(),
                values,
            },
        });
    }

    if let Some((field, op, rest)) = split_compare(body) {
        if field.is_empty() {
            return Err(ParseError {
                message: "missing field before operator".into(),
                offset,
            });
        }
        let value = parse_compare_value(rest).ok_or_else(|| ParseError {
            message: format!("expected a number after '{}', got '{rest}'", op_str(op)),
            offset,
        })?;
        return Ok(Term {
            negated,
            kind: TermKind::Compare {
                field: field.to_string(),
                op,
                value,
            },
        });
    }

    Ok(Term {
        negated,
        kind: TermKind::FreeText {
            text: body.to_string(),
        },
    })
}

/// Find the first comparison operator (two-char before one-char) and split around it.
fn split_compare(s: &str) -> Option<(&str, Cmp, &str)> {
    for (pat, op) in [(">=", Cmp::Ge), ("<=", Cmp::Le)] {
        if let Some(i) = s.find(pat) {
            return Some((&s[..i], op, &s[i + 2..]));
        }
    }
    for (pat, op) in [(">", Cmp::Gt), ("<", Cmp::Lt)] {
        if let Some(i) = s.find(pat) {
            return Some((&s[..i], op, &s[i + 1..]));
        }
    }
    None
}

/// Parse a compare value: a number with an optional trailing time unit (`ns`/`us`/`ms`/`s`),
/// scaled to nanoseconds. A bare number (no unit) is returned unchanged. Units are always
/// accepted (the grammar is field-agnostic); only `duration` gives them meaning downstream.
/// Two-char units are checked before the one-char `s` so `500ms` strips `ms`, not `s`.
fn parse_compare_value(rest: &str) -> Option<f64> {
    for (suffix, scale) in [("ns", 1.0), ("us", 1_000.0), ("ms", 1_000_000.0)] {
        if let Some(num) = rest.strip_suffix(suffix) {
            return num.parse::<f64>().ok().map(|n| n * scale);
        }
    }
    if let Some(num) = rest.strip_suffix('s') {
        return num.parse::<f64>().ok().map(|n| n * 1_000_000_000.0);
    }
    rest.parse::<f64>().ok()
}

fn op_str(op: Cmp) -> &'static str {
    match op {
        Cmp::Gt => ">",
        Cmp::Ge => ">=",
        Cmp::Lt => "<",
        Cmp::Le => "<=",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::ast::{Cmp, TermKind};

    fn kinds(input: &str) -> Vec<(bool, TermKind)> {
        parse(input)
            .unwrap()
            .terms
            .into_iter()
            .map(|t| (t.negated, t.kind))
            .collect()
    }

    #[test]
    fn empty_is_empty_query() {
        assert!(parse("").unwrap().terms.is_empty());
        assert!(parse("   ").unwrap().terms.is_empty());
    }

    #[test]
    fn field_match_single_and_list() {
        assert_eq!(
            kinds("service:api"),
            vec![(
                false,
                TermKind::Match {
                    field: "service".into(),
                    values: vec!["api".into()]
                }
            )]
        );
        assert_eq!(
            kinds("status_code:500,502"),
            vec![(
                false,
                TermKind::Match {
                    field: "status_code".into(),
                    values: vec!["500".into(), "502".into()],
                }
            )]
        );
    }

    #[test]
    fn negation_exists_compare_and_freetext() {
        assert_eq!(
            kinds("-level:debug"),
            vec![(
                true,
                TermKind::Match {
                    field: "level".into(),
                    values: vec!["debug".into()]
                }
            )]
        );
        assert_eq!(
            kinds("host.name:*"),
            vec![(
                false,
                TermKind::Exists {
                    field: "host.name".into()
                }
            )]
        );
        assert_eq!(
            kinds("status_code>=500"),
            vec![(
                false,
                TermKind::Compare {
                    field: "status_code".into(),
                    op: Cmp::Ge,
                    value: 500.0
                }
            )]
        );
        assert_eq!(
            kinds("timeout"),
            vec![(
                false,
                TermKind::FreeText {
                    text: "timeout".into()
                }
            )]
        );
    }

    #[test]
    fn quoted_phrase_keeps_spaces() {
        assert_eq!(
            kinds("\"connection refused\""),
            vec![(
                false,
                TermKind::FreeText {
                    text: "connection refused".into()
                }
            )]
        );
        // A phrase mixed with a field term, space-separated.
        assert_eq!(kinds("service:api \"a b\"").len(), 2);
    }

    #[test]
    fn errors_carry_offsets() {
        assert_eq!(parse("\"unterminated").unwrap_err().offset, 0);
        assert_eq!(
            parse("svc :x").unwrap_err().message,
            "missing field before ':'".to_string()
        );
        assert_eq!(parse("status_code>=abc").unwrap_err().offset, 0);
        // second token is the bad one; its offset points past the first token + space.
        assert_eq!(parse("ok :bad").unwrap_err().offset, 3);
    }

    #[test]
    fn compare_value_accepts_duration_units() {
        // ns/us/ms/s scale to nanoseconds; a bare number is unchanged (nanoseconds by convention).
        let v = |s: &str| match &parse(s).unwrap().terms[0].kind {
            TermKind::Compare { value, .. } => *value,
            other => panic!("expected compare, got {other:?}"),
        };
        assert_eq!(v("duration>=500"), 500.0);
        assert_eq!(v("duration>=500ns"), 500.0);
        assert_eq!(v("duration>=5us"), 5_000.0);
        assert_eq!(v("duration>=500ms"), 500_000_000.0);
        assert_eq!(v("duration>=2s"), 2_000_000_000.0);
        assert_eq!(v("duration>=1.5s"), 1_500_000_000.0);
    }

    #[test]
    fn compare_value_still_rejects_non_numeric_with_offset() {
        // A non-numeric value (even after stripping any unit) is still a parse error carrying an offset.
        assert_eq!(parse("status_code>=abc").unwrap_err().offset, 0);
        assert!(parse("duration>=ms").is_err()); // "" prefix is not a number
        assert_eq!(parse("ok status_code>=xx").unwrap_err().offset, 3);
    }
}
