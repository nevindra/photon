// Static field catalog for the spans query bar's autocomplete — the spans-mode counterpart of
// `fields.js` (see that file's header for the shared shape/rationale). Same deliberately small,
// fixed list approach: these are the fields the backend guarantees exist on every span record.
//
// Shape: `{ name, description, kind: 'match' | 'compare', values?: string[] | 'services' }`.
//   - `kind: 'match'` — `field:value` / `field:a,b` / `field:*` terms.
//   - `kind: 'compare'` — numeric comparison terms (`field>=n` / `>` / `<` / `<=`); `duration`
//     is the only one today.
//   - `values: 'services'` — the value list comes from the `services` prop (sourced from
//     `GET /api/services` at runtime; this module has no I/O and doesn't know the live list).
//   - `values: string[]` — a fixed, known enum.
//   - `values` omitted — free entry, no suggestion list.

export interface SpanFieldDescriptor {
  name: string
  description: string
  kind: 'match' | 'compare'
  values?: string[] | 'services'
}

export const SPAN_FIELDS: SpanFieldDescriptor[] = [
  {
    name: 'service',
    description: 'Service that emitted the span',
    kind: 'match',
    values: 'services',
  },
  {
    name: 'operation',
    description: 'Span operation name',
    kind: 'match',
  },
  {
    name: 'status',
    description: 'Span status code',
    kind: 'match',
    values: ['ok', 'error', 'unset'],
  },
  {
    name: 'kind',
    description: 'Span kind',
    kind: 'match',
    values: ['server', 'client', 'internal', 'producer', 'consumer'],
  },
  {
    name: 'duration',
    description: 'e.g. duration>=500ms',
    kind: 'compare',
  },
  {
    name: 'trace_id',
    description: 'Trace ID, for correlating spans across services',
    kind: 'match',
  },
  {
    name: 'span_id',
    description: 'Span ID within a trace',
    kind: 'match',
  },
  {
    name: 'parent_span_id',
    description: 'Parent span ID within a trace',
    kind: 'match',
  },
]

// name -> catalog entry, or undefined for unknown fields.
export function findSpanField(name: string): SpanFieldDescriptor | undefined {
  return SPAN_FIELDS.find((f) => f.name === name)
}

// A couple of realistic example queries for the autocomplete's empty-state (copy-in, teaches
// the syntax): a service + status match, a numeric duration compare, and a multi-field match.
export const SPAN_EXAMPLE_QUERIES: string[] = [
  'service:checkout status:error',
  'duration>=500ms',
  'operation:charge.card kind:client',
]
