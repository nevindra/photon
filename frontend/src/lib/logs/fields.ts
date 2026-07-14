// Static field catalog for the search bar's autocomplete (spec §3). This is deliberately a
// small, fixed list: `service`, `level`, `severity_text`, `trace_id`, `span_id` are the only
// fields the backend guarantees exist on every log record. Promoted/long-tail attributes
// (e.g. `status_code`) are still typeable, still lex/highlight/execute (see `queryLang.js`),
// they're just not offered as suggestions here — that waits for a fields endpoint (roadmap
// Plan 2, see the design doc's non-goals).
//
// Shape: `{ name, description, kind: 'match' | 'compare', values?: string[] | 'services' }`.
//   - `kind: 'match'` — `field:value` / `field:a,b` / `field:*` terms (all catalog fields
//     today; `'compare'` exists for future numeric-field entries and is unused so far).
//   - `values: 'services'` — the value list comes from the `services` prop (sourced from
//     `GET /api/services` at runtime; this module has no I/O and doesn't know the live list).
//   - `values: string[]` — a fixed, known enum (only `level` today).
//   - `values` omitted — free entry, no suggestion list.
import { SEVERITIES } from '@/lib/core/format'

export interface FieldDescriptor {
  name: string
  description: string
  kind: 'match' | 'compare'
  values?: string[] | 'services'
}

// Reuse the single source of truth for severity ordering/keys (`format.js`) rather than
// duplicating the debug/info/warn/error/fatal enum here.
const LEVEL_VALUES: string[] = SEVERITIES.map((s) => s.key)

export const FIELDS: FieldDescriptor[] = [
  {
    name: 'service',
    description: 'Service that emitted the log',
    kind: 'match',
    values: 'services',
  },
  {
    name: 'level',
    description: 'Log severity level',
    kind: 'match',
    values: LEVEL_VALUES,
  },
  {
    name: 'severity_text',
    description: 'Raw severity text as reported by the source',
    kind: 'match',
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
]

// name -> catalog entry, or undefined for unknown/promoted fields.
export function findField(name: string): FieldDescriptor | undefined {
  return FIELDS.find((f) => f.name === name)
}

// A couple of realistic example queries for the autocomplete's empty-state (copy-in, teaches
// the syntax): a promoted-attribute numeric compare, a negated term + quoted phrase, and a
// plain field match.
export const EXAMPLE_QUERIES: string[] = [
  'service:checkout-api status_code>=500',
  '-level:debug "timeout"',
  'trace_id:4bf92f3577b34da6a3ce929d0e0e4736',
]
