//! The UI query grammar: parse a query string into an AST, resolve field names against the
//! schema, and evaluate it in-memory. The Parquet-side compiler lives in `photon-query` (it
//! needs DataFusion); this crate stays pure so both compile targets share one source of
//! truth for filter semantics.

pub mod ast;
pub mod eval;
pub mod metric_eval;
pub mod metric_resolver;
pub mod parser;
pub mod resolver;
pub mod span_eval;
pub mod span_resolver;

pub use ast::{Cmp, Query, Term, TermKind};
pub use metric_resolver::{
    MetricFieldRef, MetricFieldResolver, MetricResolvedKind, MetricResolvedQuery,
    MetricResolvedTerm,
};
pub use parser::{parse, ParseError};
pub use resolver::{
    FieldRef, FieldResolver, ResolveError, ResolvedKind, ResolvedQuery, ResolvedTerm,
};
pub use span_resolver::{
    SpanFieldRef, SpanFieldResolver, SpanResolvedKind, SpanResolvedQuery, SpanResolvedTerm,
};
