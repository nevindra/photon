//! Raw query grammar AST — the parse tree, before field resolution.

/// A parsed query: a conjunction (AND) of terms. Empty terms = match everything.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub terms: Vec<Term>,
}

/// One term. `negated` is a leading `-` in the source and flips the whole term.
#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub negated: bool,
    pub kind: TermKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TermKind {
    /// `field:v1,v2` — field equals one of the values (OR within the field).
    Match { field: String, values: Vec<String> },
    /// `field:*` — field is present.
    Exists { field: String },
    /// `field>=n` — numeric comparison.
    Compare { field: String, op: Cmp, value: f64 },
    /// `"quoted"` or a bare word — body substring.
    FreeText { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
}
