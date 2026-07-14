// Pure, display-only lexer for the Photon query language. No Vue/DOM — this module only
// slices strings and never throws, because the common case is *partial, half-typed input*
// while the user is still composing a query.
//
// The authoritative grammar/parser lives in Rust (`crates/photon-core/src/query/parser.rs`)
// and is intentionally mirrored here term-for-term (same two-phase shape: split into
// whitespace-separated terms respecting quotes, then classify each term) so this lexer's
// notion of a "term" always matches what the backend will accept. Where the backend would
// *error* on malformed input (empty field name, unterminated quote, non-numeric compare
// value, empty value in a list), this lexer instead degrades gracefully to something
// reasonable to render — it never validates or executes, it only colors.
//
// Grammar recap (see the Rust parser's doc comments for the source of truth):
//   - Terms are whitespace-separated; a `"..."` span is one term even if it contains spaces.
//   - A term may start with `-` (negation) as long as something follows it.
//   - `field:value` / `field:a,b` (comma = OR list) / `field:*` (exists).
//   - `field>n` `field>=n` `field<n` `field<=n` (numeric compare; colon is checked first, so
//     a value like `a>b` inside `k:a>b` is a *value*, not a comparison).
//   - `"quoted phrase"` and bare words are free-text body search.
//
// Token shape: `{ start, end, text, role, negated }`.
//   - `role` is one of: 'field' | 'operator' | 'value' | 'negation' | 'freetext' | 'quoted'
//     | 'whitespace'.
//   - Negation representation (both, so the SearchBar can pick whichever is convenient):
//     the leading `-` is its own `role: 'negation'` token, AND every token that belongs to a
//     negated term — including the negation token itself — carries `negated: true`. So to
//     paint a whole negated pill with a red tint, filter on `token.negated`; to strike through
//     specifically the field key, target `token.role === 'field' && token.negated`.
//   - `tokenize()` always returns a contiguous, gap-free, non-overlapping cover of the input
//     string (every character index belongs to exactly one token), which is what an overlay
//     renderer needs — there's never an unstyled gap to fall back on.

// The role of a lexed token. See the module header for what each one paints.
export type TokenRole =
  | 'field'
  | 'operator'
  | 'value'
  | 'negation'
  | 'freetext'
  | 'quoted'
  | 'whitespace'

// One lexed token: an absolute half-open `[start, end)` slice of the query, its display `role`,
// and whether it belongs to a negated (`-field:...`) term. `tokenize()` returns a gap-free cover.
export interface Token {
  start: number
  end: number
  text: string
  role: TokenRole
  negated: boolean
}

// A raw whitespace/quote-respecting term slice, as produced by `splitTerms` (before classification).
interface RawTerm {
  text: string
  start: number
  end: number
}

// A numeric-compare operator (`findCompareOp` result).
type CompareOp = '>=' | '<=' | '>' | '<'
interface CompareMatch {
  index: number
  op: CompareOp
  len: number
}

// A char range `[start, end]`, as returned by `termRangeAt`.
export interface TermRange {
  start: number
  end: number
}

// What the autocomplete dropdown should offer at the caret — a discriminated union on `kind`.
export interface FieldCompletionContext {
  kind: 'field'
  prefix: string
}
export interface ValueCompletionContext {
  kind: 'value'
  field: string
  prefix: string
}
export interface FreetextCompletionContext {
  kind: 'freetext'
  prefix: string
}
export type CompletionContext =
  | FieldCompletionContext
  | ValueCompletionContext
  | FreetextCompletionContext

// A raw term parsed into its `field:value(s)` COLON-match shape (`parseFieldTerm`).
interface ParsedFieldTerm {
  negated: boolean
  field: string | null
  values: string[]
}

// A raw term parsed into its `field<op>value` numeric-compare shape (`parseCompareTerm`).
interface ParsedCompareTerm {
  negated: boolean
  field: string | null
}

function mk(
  start: number,
  end: number,
  text: string,
  role: TokenRole,
  negated: boolean,
): Token {
  return { start, end, text, role, negated }
}

function clamp(n: number, lo: number, hi: number): number {
  if (typeof n !== 'number' || Number.isNaN(n)) return lo
  if (n < lo) return lo
  if (n > hi) return hi
  return n
}

// Split into raw `{ text, start, end }` terms. Whitespace outside quotes separates terms; a
// `"` opens a quoted span that runs to the next `"` (or, if none exists, to the end of the
// string — an unterminated quote never errors here, it just swallows the rest of the input
// into one term, which `classifyTerm` then renders as an unterminated `quoted` token).
function splitTerms(input: string): RawTerm[] {
  const out: RawTerm[] = []
  let cur = ''
  let start = 0
  let inQuotes = false
  let have = false
  for (let i = 0; i < input.length; i++) {
    const ch = input[i]
    if (inQuotes) {
      cur += ch
      if (ch === '"') inQuotes = false
    } else if (ch === '"') {
      if (!have) {
        start = i
        have = true
      }
      cur += ch
      inQuotes = true
    } else if (/\s/.test(ch)) {
      if (have) {
        out.push({ text: cur, start, end: i })
        cur = ''
        have = false
      }
    } else {
      if (!have) {
        start = i
        have = true
      }
      cur += ch
    }
  }
  if (have) out.push({ text: cur, start, end: input.length })
  return out
}

// Find the first comparison operator in `s`, checking two-char operators before one-char
// ones — mirroring the Rust `split_compare` priority exactly (">=" is looked for anywhere in
// the string before "<=" is, which is before ">" / "<"; this is a faithful port, not a
// leftmost-in-string search).
function findCompareOp(s: string): CompareMatch | null {
  let index = s.indexOf('>=')
  if (index !== -1) return { index, op: '>=', len: 2 }
  index = s.indexOf('<=')
  if (index !== -1) return { index, op: '<=', len: 2 }
  index = s.indexOf('>')
  if (index !== -1) return { index, op: '>', len: 1 }
  index = s.indexOf('<')
  if (index !== -1) return { index, op: '<', len: 1 }
  return null
}

// Tokenize a `field:a,b,c` (or compare-value, though compare has no OR list) value span into
// alternating `value` / `operator` (comma) tokens. Empty segments (leading/trailing/doubled
// commas, e.g. mid-typing "a,,b" or "a,") are skipped — the commas themselves still tokenize
// so the overlay never has a gap.
function listTokens(s: string, base: number, negated: boolean): Token[] {
  const commas: number[] = []
  for (let i = 0; i < s.length; i++) {
    if (s[i] === ',') commas.push(i)
  }
  const tokens: Token[] = []
  let pos = 0
  for (let i = 0; i <= commas.length; i++) {
    const segEnd = i < commas.length ? commas[i] : s.length
    const segText = s.slice(pos, segEnd)
    if (segText !== '') tokens.push(mk(base + pos, base + segEnd, segText, 'value', negated))
    if (i < commas.length) {
      const c = commas[i]
      tokens.push(mk(base + c, base + c + 1, ',', 'operator', negated))
    }
    pos = segEnd + 1
  }
  return tokens
}

// Classify one raw term (as produced by `splitTerms`) into its sub-tokens, in absolute
// (whole-query) coordinates. Always returns a contiguous cover of `[absStart, absStart +
// term.length)` — no gaps, no overlaps, so the overlay can iterate the flat token list.
function classifyTerm(term: string, absStart: number): Token[] {
  const negated = term.startsWith('-') && term.length > 1
  const tokens: Token[] = []
  let bodyOffset = absStart
  let body = term
  if (negated) {
    tokens.push(mk(absStart, absStart + 1, '-', 'negation', true))
    bodyOffset = absStart + 1
    body = term.slice(1)
  }

  // Quoted phrase (terminated or not — both render as 'quoted').
  if (body.startsWith('"')) {
    tokens.push(mk(bodyOffset, bodyOffset + body.length, body, 'quoted', negated))
    return tokens
  }

  // `field:value(s)` / `field:*`. Lenient deviation from the Rust grammar: an empty field
  // name before the colon (e.g. a stray ":foo") falls through to the freetext case below
  // rather than being a dedicated error token — there's nothing meaningful to color it as.
  const colonIdx = body.indexOf(':')
  if (colonIdx > 0) {
    const field = body.slice(0, colonIdx)
    tokens.push(mk(bodyOffset, bodyOffset + colonIdx, field, 'field', negated))
    const colonAbs = bodyOffset + colonIdx
    tokens.push(mk(colonAbs, colonAbs + 1, ':', 'operator', negated))
    const valuePart = body.slice(colonIdx + 1)
    const valueStart = colonAbs + 1
    if (valuePart === '*') {
      tokens.push(mk(valueStart, valueStart + 1, '*', 'value', negated))
    } else if (valuePart !== '') {
      tokens.push(...listTokens(valuePart, valueStart, negated))
    }
    // valuePart === '' (mid-typing "field:") intentionally emits no value token — field +
    // operator already cover the whole term contiguously.
    return tokens
  }

  // Numeric compare `field>n` / `field>=n` / `field<n` / `field<=n`. Only tried when there is
  // no colon at all in the term (matches the Rust ordering exactly). Same empty-field lenient
  // fallback as above.
  const cmp = colonIdx === -1 ? findCompareOp(body) : null
  if (cmp && cmp.index > 0) {
    const field = body.slice(0, cmp.index)
    tokens.push(mk(bodyOffset, bodyOffset + cmp.index, field, 'field', negated))
    const opAbs = bodyOffset + cmp.index
    tokens.push(mk(opAbs, opAbs + cmp.len, cmp.op, 'operator', negated))
    const valuePart = body.slice(cmp.index + cmp.len)
    if (valuePart !== '') {
      tokens.push(mk(opAbs + cmp.len, opAbs + cmp.len + valuePart.length, valuePart, 'value', negated))
    }
    return tokens
  }

  // Bare word / fallback: free-text body search. This also covers the degenerate
  // empty-field-name cases above (":foo", ">5") and a lone "-" (which strips to nothing, so
  // `negated` is false and `body` is the literal "-").
  tokens.push(mk(bodyOffset, bodyOffset + body.length, body, 'freetext', negated))
  return tokens
}

// tokenize(query) -> Token[]. Never throws; coerces non-strings to ''. Returns a flat,
// contiguous, gap-free token list covering the whole input (including whitespace runs).
export function tokenize(query: string): Token[] {
  const q = typeof query === 'string' ? query : ''
  const terms = splitTerms(q)
  const tokens: Token[] = []
  let cursor = 0
  for (const term of terms) {
    if (term.start > cursor) {
      tokens.push(mk(cursor, term.start, q.slice(cursor, term.start), 'whitespace', false))
    }
    tokens.push(...classifyTerm(term.text, term.start))
    cursor = term.end
  }
  if (cursor < q.length) {
    tokens.push(mk(cursor, q.length, q.slice(cursor), 'whitespace', false))
  }
  return tokens
}

// termRangeAt(query, caret) -> { start, end } — the char range of the whole raw term (leading
// `-` and surrounding quotes included) enclosing the caret, for click-to-select-term
// (`input.setSelectionRange(start, end)`). Boundaries are inclusive on both ends (clicking
// right at the end of a pill still selects it). When the caret isn't inside any term (empty
// query, or sitting in a run of whitespace), returns a collapsed `{ start: caret, end: caret
// }` — callers never need to null-check, `setSelectionRange` with equal bounds is just a
// caret placement.
export function termRangeAt(query: string, caret: number): TermRange {
  const q = typeof query === 'string' ? query : ''
  const c = clamp(caret, 0, q.length)
  for (const term of splitTerms(q)) {
    if (c >= term.start && c <= term.end) return { start: term.start, end: term.end }
  }
  return { start: c, end: c }
}

// contextAt(query, caret) -> { kind: 'field' | 'value' | 'freetext', field?: string, prefix:
// string } — what the autocomplete dropdown should offer at the caret.
//
//   - 'field': the caret is composing a term's leading word — before any `:` or compare
//     operator has been typed (including a totally empty term, or the caret sitting in
//     whitespace / at the very start/end of the query). `prefix` is the partial word typed so
//     far. This is deliberately the fallback for *any* bare word, since until a `:`/compare
//     operator commits it, a word in progress could still become a field name.
//   - 'value': the caret is after a term's `field:` (or `field>`/`field>=`/...), including
//     right after a comma in an OR list (`,` keeps the caret in value context for the same
//     field). `field` is the literal field text typed (even if unknown/promoted — the caller
//     cross-references `fields.js` to decide what, if anything, to suggest). `prefix` is the
//     current OR-segment (text since the last comma, or since the operator if there is none).
//   - 'freetext': the caret is inside a quoted phrase (terminated or not). No field/value
//     suggestions apply; `prefix` is the quoted content typed so far (not including the
//     opening quote).
export function contextAt(query: string, caret: number): CompletionContext {
  const q = typeof query === 'string' ? query : ''
  const c = clamp(caret, 0, q.length)
  const term = splitTerms(q).find((t) => c >= t.start && c <= t.end)
  if (!term) return { kind: 'field', prefix: '' }

  const negated = term.text.startsWith('-') && term.text.length > 1
  const bodyOffset = term.start + (negated ? 1 : 0)
  const body = q.slice(bodyOffset, term.end)
  const rel = Math.max(0, c - bodyOffset)

  if (body.startsWith('"')) {
    return { kind: 'freetext', prefix: body.slice(1, rel) }
  }

  const colonIdx = body.indexOf(':')
  if (colonIdx > 0) {
    const colonAbs = bodyOffset + colonIdx
    if (c <= colonAbs) return { kind: 'field', prefix: body.slice(0, rel) }
    const field = body.slice(0, colonIdx)
    const valueText = q.slice(colonAbs + 1, c)
    const lastComma = valueText.lastIndexOf(',')
    const prefix = lastComma === -1 ? valueText : valueText.slice(lastComma + 1)
    return { kind: 'value', field, prefix }
  }

  const cmp = colonIdx === -1 ? findCompareOp(body) : null
  if (cmp && cmp.index > 0) {
    const opAbs = bodyOffset + cmp.index
    if (c <= opAbs) return { kind: 'field', prefix: body.slice(0, rel) }
    const field = body.slice(0, cmp.index)
    const prefix = q.slice(opAbs + cmp.len, c)
    return { kind: 'value', field, prefix }
  }

  return { kind: 'field', prefix: body.slice(0, rel) }
}

// ---------------------------------------------------------------------------
// Structured edit helpers. These let a UI (the FilterRail checkboxes) treat the
// query string as the single source of truth for `field:a,b` match filters:
// read the current OR values, toggle one on/off, or clear a whole field. They
// share the lexer's `field:value(s)` notion of a term so what they read/write is
// exactly what the SearchBar highlights and the backend will accept.
//
// All three are pure and never throw. They deliberately only recognise the
// COLON `field:value(s)` form (the match grammar `service`/`level` use), never
// numeric compares (`status_code>=500`) — those aren't OR lists and the rail
// doesn't drive them. Editing rebuilds the query from its whitespace-separated
// terms joined by single spaces, so inter-term whitespace is normalised (and any
// gap left by a removed term collapses) while each term's own content —
// including the internal spaces of a quoted phrase — is preserved verbatim.

// Parse a raw term (as produced by splitTerms) into { negated, field, values }.
// `field` is null unless the term is a `field:...` COLON match term (quoted,
// freetext and compare terms all yield field:null). `values` is the OR list with
// empty segments skipped, matching listTokens' leniency (so `f:a,,b` -> ['a','b'],
// `f:` -> [], `f:*` -> ['*']).
function parseFieldTerm(termText: string): ParsedFieldTerm {
  const negated = termText.startsWith('-') && termText.length > 1
  const body = negated ? termText.slice(1) : termText
  if (body.startsWith('"')) return { negated, field: null, values: [] }
  const colonIdx = body.indexOf(':')
  if (colonIdx > 0) {
    const field = body.slice(0, colonIdx)
    const values = body
      .slice(colonIdx + 1)
      .split(',')
      .filter((v) => v !== '')
    return { negated, field, values }
  }
  return { negated, field: null, values: [] }
}

// fieldValues(query, field) -> string[]. The OR values of the NON-negated
// `field:` term(s), unioned across terms, order-preserving and deduped. Returns
// [] when the field is absent. Negated terms (`-field:x`) are excluded — those
// aren't representable as a "selected" checkbox.
export function fieldValues(query: string, field: string): string[] {
  const q = typeof query === 'string' ? query : ''
  const out: string[] = []
  const seen = new Set<string>()
  for (const term of splitTerms(q)) {
    const p = parseFieldTerm(term.text)
    if (p.negated || p.field !== field) continue
    for (const v of p.values) {
      if (!seen.has(v)) {
        seen.add(v)
        out.push(v)
      }
    }
  }
  return out
}

// toggleFieldValue(query, field, value) -> string. Toggles `value` in the
// canonical set of the NON-negated `field:` values (the same union `fieldValues`
// reads), then rewrites the query so there is exactly ONE `field:` term carrying
// that set — collapsing any duplicate `field:` terms in the process. This keeps
// the writer and `fieldValues` (the reader) in lock-step, so a checkbox derived
// from `fieldValues` can never desync from what a toggle produced, even for a
// hand-typed `service:a service:b`. The surviving term keeps the position of the
// first `field:` term (or is appended if none existed). All other terms (free
// text, quoted phrases, other fields, negated terms) are preserved; the result
// has single-space separators and no leading/trailing whitespace.
export function toggleFieldValue(query: string, field: string, value: string): string {
  const q = typeof query === 'string' ? query : ''
  const current = fieldValues(q, field)
  const next = current.includes(value)
    ? current.filter((v) => v !== value)
    : [...current, value]

  const terms = splitTerms(q).map((t) => t.text)
  let firstIdx = -1
  for (let i = 0; i < terms.length; i++) {
    const p = parseFieldTerm(terms[i])
    if (!p.negated && p.field === field) {
      firstIdx = i
      break
    }
  }

  const canonical = next.length > 0 ? `${field}:${next.join(',')}` : null
  const out: string[] = []
  for (let i = 0; i < terms.length; i++) {
    const p = parseFieldTerm(terms[i])
    const isPositiveField = !p.negated && p.field === field
    if (i === firstIdx) {
      if (canonical) out.push(canonical) // place the collapsed set where the first term was
    } else if (isPositiveField) {
      continue // drop duplicate positive `field:` terms (folded into `canonical`)
    } else {
      out.push(terms[i]) // preserve everything else verbatim
    }
  }
  if (firstIdx === -1 && canonical) out.push(canonical) // no prior term → append
  return out.join(' ')
}

// toggleFieldValueNegated(query, field, value) -> string. The filter-OUT
// counterpart to `toggleFieldValue`: toggle the presence of the exact
// `-field:value` negated term. Appends `-field:value` when that term is absent;
// removes it when present; every other term (free text, quoted phrases, other
// fields, the POSITIVE `field:value` term, other negated terms) is preserved
// verbatim. A positive `field:value` term is deliberately distinct from
// `-field:value` — toggling one never touches the other. Like the sibling
// helpers it rebuilds from whitespace-separated terms joined by single spaces
// (whitespace normalised), is pure and never throws.
export function toggleFieldValueNegated(query: string, field: string, value: string): string {
  const q = typeof query === 'string' ? query : ''
  const terms = splitTerms(q).map((t) => t.text)
  const isTarget = (t: string): boolean => {
    const p = parseFieldTerm(t)
    return p.negated && p.field === field && p.values.length === 1 && p.values[0] === value
  }
  if (terms.some(isTarget)) {
    return terms.filter((t) => !isTarget(t)).join(' ')
  }
  return [...terms, `-${field}:${value}`].join(' ')
}

// removeField(query, field) -> string. Remove every non-negated `field:` term
// (for a "clear" action), preserving all other terms — including negated
// `-field:x` terms, which the rail doesn't own — and normalising whitespace to
// single-space separators with no leading/trailing whitespace.
export function removeField(query: string, field: string): string {
  const q = typeof query === 'string' ? query : ''
  return splitTerms(q)
    .map((t) => t.text)
    .filter((t) => {
      const p = parseFieldTerm(t)
      return p.negated || p.field !== field
    })
    .join(' ')
}

// removeFieldAll(query, field) -> string. Remove EVERY term of `field` — positive
// `field:` AND negated `-field:` — resetting it to the default (all-included) state.
// Unlike removeField (which keeps negated terms), this drops both. Used by "Clear All"
// and by facet-count stripping.
export function removeFieldAll(query: string, field: string): string {
  const q = typeof query === 'string' ? query : ''
  return splitTerms(q)
    .map((t) => t.text)
    .filter((t) => parseFieldTerm(t).field !== field) // drops both signs of `field:`
    .join(' ')
}

// onlyFieldValue(query, field, value) -> string. Narrow `field` to exactly
// `field:value`: drop ALL of the field's positive AND negated terms, then set the
// single include. Preserves all other fields' terms verbatim.
export function onlyFieldValue(query: string, field: string, value: string): string {
  const kept = removeFieldAll(query, field)
  return kept ? `${kept} ${field}:${value}` : `${field}:${value}`
}

// negatedFieldValues(query, field) -> string[]. The counterpart to fieldValues:
// the values of the NEGATED `-field:` term(s), unioned, order-preserving, deduped.
// Lets a facet rail pin/mark excluded values the way fieldValues surfaces selected
// ones.
export function negatedFieldValues(query: string, field: string): string[] {
  const q = typeof query === 'string' ? query : ''
  const out: string[] = []
  const seen = new Set<string>()
  for (const term of splitTerms(q)) {
    const p = parseFieldTerm(term.text)
    if (!p.negated || p.field !== field) continue
    for (const v of p.values) {
      if (!seen.has(v)) {
        seen.add(v)
        out.push(v)
      }
    }
  }
  return out
}

// ---------------------------------------------------------------------------
// Single-state facet helpers (SigNoz-style). Each facet value has exactly ONE
// state — checked (in the result set) or unchecked (out) — so include-and-exclude
// of the same value is impossible by construction. Two modes fall out of the
// grammar (callers never branch on this — these helpers do): all-mode (no
// positive `field:` includes — every value checked except `-field:` exclusions)
// and include-mode (positive `field:` includes exist — only those are checked).

// facetChecked(query, field, value) -> boolean. Single-state checkbox state:
//   include-mode (positive includes exist) -> checked iff value is included.
//   all-mode -> checked unless the value is excluded (so default = all checked).
export function facetChecked(query: string, field: string, value: string): boolean {
  const inc = fieldValues(query, field)
  if (inc.length > 0) return inc.includes(value)
  return !negatedFieldValues(query, field).includes(value)
}

// toggleFacetValue(query, field, value) -> string. Mode-aware single-state toggle:
//   include-mode -> toggle positive membership (toggleFieldValue).
//   all-mode     -> toggle exclusion (toggleFieldValueNegated).
// Emptying the include set returns the field to all-mode (all checked); emptying the
// exclusion set returns it to the pristine default. Never yields field:v AND -field:v.
export function toggleFacetValue(query: string, field: string, value: string): string {
  if (fieldValues(query, field).length > 0) return toggleFieldValue(query, field, value)
  return toggleFieldValueNegated(query, field, value)
}

// fieldConstraintCount(query, field) -> number. How many terms constrain `field`
// (positive includes + negated excludes). Drives the "N" badge and Clear-All visibility.
export function fieldConstraintCount(query: string, field: string): number {
  return fieldValues(query, field).length + negatedFieldValues(query, field).length
}

// ---------------------------------------------------------------------------
// Compare-term structured-edit helpers. The colon helpers above deliberately
// handle only `field:value(s)` match terms; these are their `field>`/`field>=`/
// `field</`field<=` compare-term counterparts — the numeric-range form the
// latency-histogram duration brush writes. Pure, never throw, same
// whitespace-normalising rebuild.

// Parse a raw term into { negated, field } where `field` is the compared field
// name IFF the term is a `field<op>value` numeric compare (a colon term, quoted
// phrase or freetext all yield field:null — colon wins over compare, matching
// the lexer's ordering in classifyTerm).
function parseCompareTerm(termText: string): ParsedCompareTerm {
  const negated = termText.startsWith('-') && termText.length > 1
  const body = negated ? termText.slice(1) : termText
  if (body.startsWith('"')) return { negated, field: null }
  const colonIdx = body.indexOf(':')
  if (colonIdx > 0) return { negated, field: null }
  const cmp = colonIdx === -1 ? findCompareOp(body) : null
  if (cmp && cmp.index > 0) return { negated, field: body.slice(0, cmp.index) }
  return { negated, field: null }
}

// durationLiteral(ns) -> string. Format a nanosecond count as the SHORTEST
// grammar-valid duration literal, choosing the largest unit whose mantissa is
// >= 1 and rounding that mantissa to ~3 significant digits so the pill stays
// short (float round-trip is fine for filtering). Emits `us` — never the micro
// sign `µs`, which the grammar rejects — so this is a SEPARATE formatter, not a
// reuse of format.js's `formatDuration`.
function round3(n: number): number {
  if (!Number.isFinite(n) || n === 0) return 0
  return Number(n.toPrecision(3))
}
export function durationLiteral(ns: number): string {
  const n = Number(ns) || 0
  if (n >= 1e9) return `${round3(n / 1e9)}s`
  if (n >= 1e6) return `${round3(n / 1e6)}ms`
  if (n >= 1e3) return `${round3(n / 1e3)}us`
  return `${round3(n)}ns`
}

// removeCompareField(query, field) -> string. Remove every non-negated compare
// term for `field` (`field>`/`field>=`/`field<`/`field<=`), preserving all other
// terms — including negated `-field>=x` compares, which the brush doesn't own,
// and colon `field:` match terms of the same name — and normalising whitespace to
// single-space separators. The compare-term analog of `removeField`.
export function removeCompareField(query: string, field: string): string {
  const q = typeof query === 'string' ? query : ''
  return splitTerms(q)
    .map((t) => t.text)
    .filter((t) => {
      const c = parseCompareTerm(t)
      return c.negated || c.field !== field
    })
    .join(' ')
}

// setDurationRange(query, minNs, maxNs) -> string. Replace any existing
// `duration` compare range (no stacking) with `duration>=<min> duration<=<max>`,
// omitting the min bound when `minNs <= 0` (a selection anchored at the first
// bucket). Returns the query text to assign to the search box.
export function setDurationRange(query: string, minNs: number, maxNs: number): string {
  const base = removeCompareField(query, 'duration')
  const parts: string[] = []
  if (Number(minNs) > 0) parts.push(`duration>=${durationLiteral(minNs)}`)
  parts.push(`duration<=${durationLiteral(maxNs)}`)
  const suffix = parts.join(' ')
  return base ? `${base} ${suffix}` : suffix
}
