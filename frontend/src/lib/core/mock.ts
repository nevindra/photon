// A realistic-feeling in-browser dataset so the UI is fully interactive before the
// backend is wired. Timestamps are epoch nanoseconds (BigInt), matching the API.

import { fieldValues, removeField } from '@/lib/core/queryLang'
import type { InfraHost, InfraHostsResult, InfraHostDetail, InfraSeriesResult } from '@/lib/core/api'
import type {
  AlertRule,
  AlertRuleInput,
  AlertChannel,
  AlertChannelInput,
  AlertIncident,
  AlertIncidentsFilter,
  AlertCondition,
  AlertPreviewResult,
  AlertPreviewSeries,
  AlertRuleResult,
  AlertChannelResult,
  AlertTestRuleResult,
  MutationResult,
} from '@/lib/core/api'

export const SERVICES = ['api', 'web', 'worker', 'auth-svc', 'ingestor', 'postgres']

// The severity levels used across the corpus and the histogram buckets.
export const SEVERITY_KEYS = ['debug', 'info', 'warn', 'error', 'fatal']

const MS = 1_000_000n // ns per ms

// ---------------------------------------------------------------------------
// Shared payload/response types. The EXPORTED mock functions return these; they
// mirror the real `/api/*` JSON shapes so `api.ts`'s offline fallback is shape-
// identical to the server. The internal fixture/row builders below lean on `any`
// where precise typing would cost more than it's worth.
// ---------------------------------------------------------------------------

export type SeverityKey = 'debug' | 'info' | 'warn' | 'error' | 'fatal'
export type StatusKeyword = 'ok' | 'error' | 'unset'

// --- Logs ---
export interface MockLogRow {
  id: number
  timestamp: bigint
  severity: string
  service: string
  body: string
  trace_id: string | null
  span_id: string | null
  attributes: Record<string, string>
}

export interface SearchRequest {
  services?: string[] | null
  severities?: string[] | null
  text?: string | null
  query?: string | null
  start?: string
  end?: string
  limit?: number
}

export interface SearchEnvelope {
  rows: MockLogRow[]
  matched_count: number
  elapsed_ms: number
}

export interface FieldEntry {
  name: string
  kind: 'fixed' | 'promoted' | 'attribute'
}

export interface FacetValue {
  value: string
  count: number
}

export interface FacetResult {
  values: FacetValue[]
  capped: boolean
}

export interface LogHistogramBucket {
  t: string
  debug: number
  info: number
  warn: number
  error: number
  fatal: number
  total: number
}

// --- Traces / spans ---
export interface MockSpan {
  trace_id: string
  span_id: string
  parent_span_id: string | null
  name: string
  kind: number
  kind_text: string
  start_time_nanos: string
  end_time_nanos: string
  duration_nanos: number
  status_code: number
  status_text: string
  status_message: string | null
  scope_name: string
  service: string
  events: any[] | null
  links: null
  attributes: Record<string, string>
}

export interface MockTrace {
  trace_id: string
  spans: MockSpan[]
}

export interface TraceEnvelope {
  trace_id: string
  spans: MockSpan[]
  elapsed_ms: number
}

export interface SpanSearchRequest {
  query?: string
  sort?: string
  limit?: number
  cursor?: string
}

export interface SpanSearchEnvelope {
  rows: MockSpan[]
  matched_count: number
  elapsed_ms: number
  next_cursor?: string
}

export interface TraceSummary {
  trace_id: string
  root_service: string | null
  root_name: string | null
  start_ts: string
  duration_ns: string | null
  span_count: number
  error_count: number
  services: string[]
}

export interface TraceSearchRequest {
  query?: string
  sort?: string
  limit?: number
  cursor?: string
}

export interface TraceSearchEnvelope {
  traces: TraceSummary[]
  matched_count: number
  elapsed_ms: number
  next_cursor?: string
}

export interface TraceHistogramBucket {
  t: string
  ok: number
  error: number
  unset: number
  total: number
}

export interface LatencyBucket {
  bucket_ns: string
  count: number
}

export interface LatencyResult {
  buckets: LatencyBucket[]
  p50: string
  p90: string
  p99: string
}

export interface RedRow {
  service: string
  operation: string | null
  count: number
  rate: number
  error_count: number
  error_rate: number
  p50: string
  p90: string
  p99: string
}

export interface ServiceTimeseriesPoint {
  ts: number
  rate: number
  error_rate: number
  p50: string
  p90: string
  p99: string
}

// --- Metrics ---
export interface MetricCatalogEntry {
  name: string
  type: string
  unit: string
  temporality: string | null
  is_monotonic: boolean | null
  series_count: number
  last_seen: string
}

export interface MetricMetadata {
  name: string
  type: string
  temporality: string | null
  is_monotonic: boolean | null
  unit: string
  series_count: number
  last_seen: string
  attribute_keys: string[]
}

export type MetricLabelsResult = { keys: string[] } | { values: string[]; capped: boolean }

export interface MetricQuerySpec {
  id: string
  metric: string
  group_by?: string[] | null
  filter?: string
}

export interface MetricQueryRequest {
  queries: MetricQuerySpec[]
  start: string
  end: string
}

export interface MetricPoint {
  t: string
  v: number | null
}

export interface MetricSeries {
  labels: Record<string, string | null>
  points: MetricPoint[]
  exemplars: unknown[]
}

export interface MetricQueryResult {
  id: string
  series: MetricSeries[]
  default_agg: string
}

export interface MetricQueryResponse {
  results: MetricQueryResult[]
  step: string
  capped: boolean
  elapsed_ms: number
}

// --- Data & usage ---
export interface UsagePoint {
  ts: number
  hot_bytes: number
  durable_bytes: number
  total_rows: number
  ingest_rows: number | null
  ingest_bytes: number | null
}

export interface UsageSeriesResponse {
  window: string
  bucket_ms: number
  series: Record<string, UsagePoint[]>
}

// --- RUM ---
export interface RumApp {
  name: string
  key: string
  allowed_origins: string[]
  sample_rate: number
  rate_limit: number
  created_at: number
}

export interface RumVitalRow {
  metric: string
  p75: number
  rating: string
  good_max: number
  poor_min: number
  dist: { good: number; needs: number; poor: number; total: number }
}

export interface RumBreakdownRow {
  key: string
  pageviews: number
  lcp_p75: number
  inp_p75: number
  cls_p75: number
}

export interface RumPageRow {
  route: string
  pageviews: number
  lcp_p75: number
  inp_p75: number
  cls_p75: number
}

export interface RumPublicError {
  fingerprint: string
  exception_type: string
  message: string
  count: number
  sessions: number
}

export interface RumLcpAttribution {
  ttfb: number | null
  resource_load_delay: number | null
  resource_load_time: number | null
  element_render_delay: number | null
  element: string | null
}

export interface RumPageDetail {
  app: string
  route: string
  vitals: { pageviews: number; lcp_p75: number; inp_p75: number; cls_p75: number } | null
  breakdown: RumBreakdownRow[]
  errors: RumPublicError[]
  attribution: { lcp: RumLcpAttribution }
}

export interface RumErrorEvent {
  timestamp: number
  route: string
  browser: string
  device: string
  session: string
  trace_id?: string | null
}
export interface RumErrorTagValue {
  value: string
  count: number
}
export interface RumErrorTag {
  field: string
  values: RumErrorTagValue[]
}
export interface RumErrorCountBucket {
  t: number
  count: number
}
export interface RumErrorDetail {
  app: string
  fingerprint: string
  exception_type: string
  message: string
  error_kind: string
  first_seen: number
  last_seen: number
  occurrences: number
  sessions: number
  series: RumErrorCountBucket[]
  tags: RumErrorTag[]
  sample_stack: string | null
  events: RumErrorEvent[]
}
export interface RumErrorFacetsResult {
  app: string
  facets: Record<string, { values: RumErrorTagValue[]; capped: boolean }>
}

// --- Internal-only fixture shapes ---
interface MetricDef {
  name: string
  type: string
  unit: string
  temporality: string | null
  is_monotonic: boolean | null
  keys: string[]
}

interface RedGroup {
  service: string
  operation: string | null
  count: number
  error_count: number
  durations: number[]
}

interface DurationTerm {
  full: string
  negated: boolean
  op: string
  value: number
}

interface RumErrorRow extends RumPublicError {
  route: string
}

// weighted severity mix — mostly info, a long tail of trouble
const MIX: [SeverityKey, number][] = [
  ['info', 60],
  ['debug', 14],
  ['warn', 14],
  ['error', 10],
  ['fatal', 2],
]

const BODIES: Record<SeverityKey, string[]> = {
  debug: [
    'cache hit for key user:{id}',
    'span exported to collector',
    'config reloaded from disk',
    'connection pool size now {n}',
  ],
  info: [
    'GET /health 200 in {n}ms',
    'POST /v1/logs accepted {n} records',
    'indexing batch {n} complete',
    'session established for tenant acme',
    'compaction sealed segment {n}',
    'request served in {n}ms',
  ],
  warn: [
    'retry {n}/3 to upstream payments',
    'slow query took {n}ms',
    'replication lag {n}s and rising',
    'WAL quota at {n}% of local disk',
  ],
  error: [
    'error when indexing document {id}: schema mismatch',
    'connection timeout to 10.0.0.{n}:5432',
    'error when indexing batch {n}: field overflow',
    'failed to flush segment {n}: disk full',
    'unhandled rejection in worker {n}',
  ],
  fatal: [
    'panic: object store unreachable, shutting down',
    'fatal: WAL fsync failed, refusing writes',
  ],
}

function pick<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)]
}

function weightedSeverity(): SeverityKey {
  const total = MIX.reduce((a, [, w]) => a + w, 0)
  let r = Math.random() * total
  for (const [k, w] of MIX) {
    if ((r -= w) <= 0) return k
  }
  return 'info'
}

function fill(t: string): string {
  return t
    .replace('{n}', String(Math.floor(Math.random() * 900 + 8)))
    .replace('{id}', Math.random().toString(36).slice(2, 10))
}

function hex(len: number): string {
  let s = ''
  for (let i = 0; i < len; i++) s += Math.floor(Math.random() * 16).toString(16)
  return s
}

// Generate the corpus once per session.
export const WINDOW_MS = 30 * 60 * 1000
const NOW = Date.now()
let seq = 0

export const RECORDS: MockLogRow[] = Array.from({ length: 520 }, (): MockLogRow => {
  const sev = weightedSeverity()
  const service = pick(SERVICES)
  const offset = Math.floor(Math.random() * WINDOW_MS)
  const tsMs = NOW - offset
  return {
    id: seq++,
    timestamp: BigInt(tsMs) * MS,
    severity: sev,
    service,
    body: fill(pick(BODIES[sev])),
    trace_id: Math.random() > 0.4 ? hex(32) : null,
    span_id: Math.random() > 0.4 ? hex(16) : null,
    attributes: {
      'host.name': `${service}-${Math.floor(Math.random() * 4) + 1}`,
      'deployment.environment': 'production',
      region: pick(['us-east-1', 'eu-west-1', 'ap-south-1']),
    },
  }
}).sort((a, b) => (b.timestamp > a.timestamp ? 1 : -1))

// Filter the corpus the way the backend's search would. Since the rail now writes
// service/severity filters into the grammar query (e.g. `service:api level:error`)
// rather than the structured lists, the mock parses those out of the query too —
// otherwise rail clicks would be silent no-ops in dev/mock mode. Structured lists
// still win if a caller supplies them. Returns the FULL matched array (no limit) so
// queryMock / mockFacet / mockHistogram can all share one filter pass.
function filterRecords({ services, severities, text, query }: SearchRequest): MockLogRow[] {
  const raw = (query ?? text ?? '').trim()
  const svcList = services && services.length ? services : fieldValues(raw, 'service')
  const sevList = severities && severities.length ? severities : fieldValues(raw, 'level')
  const svc = svcList.length ? new Set(svcList) : null
  const sev = sevList.length ? new Set(sevList) : null
  // Free text = the query with the service:/level: terms stripped; only substring-
  // match it when what remains is plain words (no other grammar metachars).
  const freeText = removeField(removeField(raw, 'service'), 'level').trim()
  const q = /[:<>"]/.test(freeText) ? '' : freeText.toLowerCase()
  return RECORDS.filter((r) => {
    if (svc && !svc.has(r.service)) return false
    if (sev && !sev.has(r.severity)) return false
    if (q && !r.body.toLowerCase().includes(q)) return false
    return true
  })
}

// queryMock: returns the search envelope { rows, matched_count, elapsed_ms }, matching
// the real POST /api/search shape so the api.js fallback is byte-identical.
export function queryMock(request: SearchRequest): SearchEnvelope {
  const matched = filterRecords(request)
  const limit = request.limit ?? 500
  return {
    rows: matched.slice(0, limit),
    matched_count: matched.length,
    elapsed_ms: 3, // deterministic stub
  }
}

// Fixed + promoted + the distinct attribute keys present in the mock corpus.
export function mockFields(): FieldEntry[] {
  const FIXED = [
    'timestamp',
    'observed_timestamp',
    'severity_number',
    'severity_text',
    'body',
    'trace_id',
    'span_id',
    'scope_name',
  ]
  const PROMOTED = ['service.name']
  const attrKeys = new Set<string>()
  for (const r of RECORDS) {
    for (const k of Object.keys(r.attributes ?? {})) attrKeys.add(k)
  }
  return [
    ...FIXED.map((name): FieldEntry => ({ name, kind: 'fixed' })),
    ...PROMOTED.map((name): FieldEntry => ({ name, kind: 'promoted' })),
    ...[...attrKeys].sort().map((name): FieldEntry => ({ name, kind: 'attribute' })),
  ]
}

// Top values + counts for one field over the filtered corpus, sorted by count desc.
export function mockFacet(field: string, query: string, start: string, end: string, limit = 50): FacetResult {
  const matched = filterRecords({ query, start, end, limit: RECORDS.length })
  const counts = new Map<string, number>()
  for (const r of matched) {
    // The pinned Logs sections facet the real columns — Services on `service.name`, Severity on
    // `severity_text` (folded to lower-case keys downstream) — so map those to the record's
    // promoted fields the way `mockTracesFacet` does; everything else is a long-tail attribute.
    const value =
      field === 'service' || field === 'service.name'
        ? r.service
        : field === 'severity_text'
          ? r.severity
          : r.attributes?.[field]
    if (value == null) continue
    counts.set(value, (counts.get(value) ?? 0) + 1)
  }
  const values = [...counts.entries()]
    .map(([value, count]) => ({ value, count }))
    .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value))
  return { values: values.slice(0, limit), capped: values.length > limit }
}

// Severity-stacked histogram over [start, end] (nanos strings) with `buckets`
// equal-width bins. Supersedes the old fixed-window `bucketize`.
export function mockHistogram(query: string, start: string, end: string, buckets = 48): LogHistogramBucket[] {
  const NS = 1_000_000n
  const startMs = Number(BigInt(start) / NS)
  const endMs = Number(BigInt(end) / NS)
  const n = Math.max(1, buckets)
  const width = Math.max(1, (endMs - startMs) / n)
  const out: LogHistogramBucket[] = Array.from({ length: n }, (_, i) => ({
    t: (BigInt(Math.round(startMs + i * width)) * NS).toString(),
    debug: 0,
    info: 0,
    warn: 0,
    error: 0,
    fatal: 0,
    total: 0,
  }))
  const matched = filterRecords({ query, start, end, limit: RECORDS.length })
  for (const r of matched) {
    const ms = Number(r.timestamp / NS)
    let i = Math.floor((ms - startMs) / width)
    if (i < 0) i = 0
    if (i >= n) i = n - 1
    const key = (SEVERITY_KEYS.includes(r.severity) ? r.severity : 'info') as SeverityKey
    out[i][key]++
    out[i].total++
  }
  return out
}

// A realistic multi-service trace for dev/mock mode: a root server span with a fan-out of
// client/db children across services, one errored leaf, and one span carrying an event.
// Times are decimal-nanosecond STRINGS (API-faithful); api.getTrace hydrates them to BigInt.
export function mockTrace(traceId?: string): TraceEnvelope {
  const tid = traceId || hex(32)
  const base = BigInt(Date.now()) * MS // trace start, ns
  const ns = (offsetMs: number, durMs: number) => ({
    start: (base + BigInt(offsetMs) * MS).toString(),
    end: (base + BigInt(offsetMs + durMs) * MS).toString(),
    dur: durMs * 1_000_000,
  })
  const mk = (
    span_id: string,
    parent_span_id: string | null,
    service: string,
    name: string,
    offsetMs: number,
    durMs: number,
    extra: any = {},
  ): MockSpan => {
    const t = ns(offsetMs, durMs)
    return {
      trace_id: tid,
      span_id,
      parent_span_id,
      name,
      kind: extra.kind ?? 1,
      kind_text: extra.kind_text ?? 'SERVER',
      start_time_nanos: t.start,
      end_time_nanos: t.end,
      duration_nanos: t.dur,
      status_code: extra.status_code ?? 0,
      status_text: extra.status_text ?? 'UNSET',
      status_message: extra.status_message ?? null,
      scope_name: service,
      service,
      events: extra.events ?? null,
      links: null,
      attributes: extra.attributes ?? { 'deployment.environment': 'production' },
    }
  }
  const spans = [
    mk('s1', null, 'web', 'GET /checkout', 0, 240, { kind_text: 'SERVER' }),
    mk('s2', 's1', 'api', 'POST /orders', 15, 200, { kind: 3, kind_text: 'CLIENT' }),
    mk('s3', 's2', 'api', 'validate.cart', 20, 30),
    mk('s4', 's2', 'postgres', 'SELECT orders', 60, 90, { kind: 3, kind_text: 'CLIENT' }),
    mk('s5', 's2', 'payments', 'charge.card', 120, 85, {
      kind: 3,
      kind_text: 'CLIENT',
      events: [{ name: 'retry', time_unix_nano: (base + 140n * MS).toString(), attributes: {} }],
    }),
    mk('s6', 's5', 'payments', 'gateway.authorize', 130, 60, {
      status_code: 2,
      status_text: 'ERROR',
      status_message: 'card declined',
    }),
    mk('s7', 's1', 'worker', 'emit.receipt', 210, 25),
  ]
  return { trace_id: tid, spans, elapsed_ms: 2 }
}

// ---------------------------------------------------------------------------
// Traces / spans corpus (Phase 4 Traces Explorer). A small multi-trace corpus —
// each trace a shallow parent/child tree across 2-4 of the existing SERVICES —
// generated once per session, like RECORDS above. Spans use the exact shape
// `mockTrace` produces (decimal-nanosecond STRINGS for start/end, a plain
// number for duration_nanos) so `mockSearchSpans` rows are byte-identical to
// `api.getTrace`'s spans and hydrate the same way.

const TRACE_OPERATIONS: Record<string, string[]> = {
  api: ['POST /orders', 'GET /orders/{id}', 'validate.cart'],
  web: ['GET /checkout', 'GET /cart', 'GET /health'],
  worker: ['emit.receipt', 'process.queue', 'compaction.run'],
  'auth-svc': ['POST /token', 'verify.session'],
  ingestor: ['ingest.batch', 'flush.segment'],
  postgres: ['SELECT orders', 'INSERT orders', 'UPDATE inventory'],
}

// OTLP status codes: 0 unset, 1 ok, 2 error (mirrors the backend's `status_codes`).
function weightedSpanStatus(): { code: number; text: string } {
  const r = Math.random()
  if (r < 0.08) return { code: 2, text: 'ERROR' } // long tail of errored spans
  if (r < 0.55) return { code: 1, text: 'OK' }
  return { code: 0, text: 'UNSET' }
}

function opFor(service: string): string {
  return pick(TRACE_OPERATIONS[service] ?? ['handle'])
}

const TRACE_WINDOW_MS = 30 * 60 * 1000
const TRACE_NOW = Date.now()

function genSpan(
  traceId: string,
  spanId: string,
  parentSpanId: string | null,
  service: string,
  name: string,
  startMs: number,
  durMs: number,
): MockSpan {
  const startNs = BigInt(startMs) * MS
  const endNs = startNs + BigInt(durMs) * MS
  const status = weightedSpanStatus()
  return {
    trace_id: traceId,
    span_id: spanId,
    parent_span_id: parentSpanId,
    name,
    kind: parentSpanId ? 3 : 1,
    kind_text: parentSpanId ? 'CLIENT' : 'SERVER',
    start_time_nanos: startNs.toString(),
    end_time_nanos: endNs.toString(),
    duration_nanos: durMs * 1_000_000,
    status_code: status.code,
    status_text: status.text,
    status_message: status.code === 2 ? 'downstream error' : null,
    scope_name: service,
    service,
    events: null,
    links: null,
    attributes: { 'deployment.environment': 'production' },
  }
}

// One trace: a root span (a random service) plus 1-3 child spans on other services. ~15% of
// traces run slow (a long root span) so `sort=slowest` and duration filters have something to
// find; per-span status is weighted so ~8% of spans (and therefore a chunk of traces) error out.
function genTrace(): MockTrace {
  const traceId = hex(32)
  const rootService = pick(SERVICES)
  const others = SERVICES.filter((s) => s !== rootService)
  const nChildren = 1 + Math.floor(Math.random() * 3) // 1..3
  const childServices = Array.from({ length: nChildren }, () => pick(others))
  const offset = Math.floor(Math.random() * TRACE_WINDOW_MS)
  const startMs = TRACE_NOW - offset
  const slow = Math.random() < 0.15
  const rootDur = slow ? 800 + Math.floor(Math.random() * 1200) : 20 + Math.floor(Math.random() * 300)
  const rootId = hex(16)
  const spans = [genSpan(traceId, rootId, null, rootService, opFor(rootService), startMs, rootDur)]
  let cursor = 5
  for (const svc of childServices) {
    const childDur = Math.max(5, Math.floor(rootDur / (nChildren + 1)) + Math.floor(Math.random() * 40))
    spans.push(genSpan(traceId, hex(16), rootId, svc, opFor(svc), startMs + cursor, childDur))
    cursor += Math.max(5, Math.floor(childDur / 3))
  }
  return { trace_id: traceId, spans }
}

const TRACES = Array.from({ length: 40 }, () => genTrace())
const TRACE_SPANS = TRACES.flatMap((t) => t.spans)

// Compare two BigInts for a "most recent first" sort: negative means `a` sorts first.
function cmpBigDesc(a: bigint, b: bigint): number {
  return a > b ? -1 : a < b ? 1 : 0
}

// A `field><=>=n(unit)?` compare term for `duration` — the one grammar shape `fieldValues`/
// `removeField` don't cover (they only understand `field:value` match terms). Mirrors the
// backend's unit handling: ns/us/ms/s scale to nanoseconds, a bare number is nanoseconds as-is.
const DURATION_RE = /(?:^|\s)(-)?duration(>=|<=|>|<)([0-9.]+)(ns|us|ms|s)?(?=\s|$)/
const DURATION_UNIT_SCALE: Record<string, number> = { ns: 1, us: 1e3, ms: 1e6, s: 1e9 }

function parseDurationTerm(raw: string): DurationTerm | null {
  const m = DURATION_RE.exec(raw)
  if (!m) return null
  const [full, neg, op, numStr, unit] = m
  const scale = unit ? DURATION_UNIT_SCALE[unit] : 1
  return { full, negated: !!neg, op, value: parseFloat(numStr) * scale }
}

function durationCompare(op: string, a: number, b: number): boolean {
  switch (op) {
    case '>':
      return a > b
    case '>=':
      return a >= b
    case '<':
      return a < b
    case '<=':
      return a <= b
    default:
      return true
  }
}

function statusKeyword(code: number): StatusKeyword {
  return code === 2 ? 'error' : code === 1 ? 'ok' : 'unset'
}

// Filter the spans corpus the way the backend's spans grammar would, for the subset the mock
// supports: service:/status: match lists (via the shared lexer helpers), a duration compare, and
// a free-text substring over the span name (operation). Time window (`start`/`end`) is not
// applied — like `filterRecords` above, the corpus is generated to already sit inside the
// windows the UI asks for. Returns the full matched array (no limit).
function filterSpans({ query }: { query?: string } = {}): MockSpan[] {
  const raw = (query ?? '').trim()
  const svcList = fieldValues(raw, 'service')
  const statusList = fieldValues(raw, 'status')
  const svc = svcList.length ? new Set(svcList) : null
  const status = statusList.length ? new Set(statusList) : null
  const durTerm = parseDurationTerm(raw)

  let stripped = removeField(removeField(raw, 'service'), 'status')
  if (durTerm) stripped = stripped.replace(durTerm.full, ' ').replace(/\s+/g, ' ').trim()
  const freeText = /[:<>"]/.test(stripped) ? '' : stripped.toLowerCase()

  return TRACE_SPANS.filter((s) => {
    if (svc && !svc.has(s.service)) return false
    if (status && !status.has(statusKeyword(s.status_code))) return false
    if (durTerm) {
      const ok = durationCompare(durTerm.op, s.duration_nanos, durTerm.value)
      if (durTerm.negated ? ok : !ok) return false
    }
    if (freeText && !(s.name ?? '').toLowerCase().includes(freeText)) return false
    return true
  })
}

function sortSpans(spans: MockSpan[], sort?: string): MockSpan[] {
  const arr = [...spans]
  if (sort === 'slowest') {
    arr.sort((a, b) => (b.duration_nanos ?? -1) - (a.duration_nanos ?? -1))
  } else if (sort === 'errors') {
    arr.sort((a, b) => {
      const ae = a.status_code === 2 ? 0 : 1
      const be = b.status_code === 2 ? 0 : 1
      return ae - be || cmpBigDesc(BigInt(a.start_time_nanos), BigInt(b.start_time_nanos))
    })
  } else {
    arr.sort((a, b) => cmpBigDesc(BigInt(a.start_time_nanos), BigInt(b.start_time_nanos)))
  }
  return arr
}

// mockSearchSpans: returns the search envelope { rows, matched_count, elapsed_ms, next_cursor? },
// matching the real POST /api/spans/search shape (span rows use the same shape as `mockTrace`'s
// spans / `api.getTrace`, so `hydrateSpans` works unchanged).
export function mockSearchSpans({ query, sort, limit, cursor }: SpanSearchRequest = {}): SpanSearchEnvelope {
  const matched = sortSpans(filterSpans({ query }), sort)
  const lim = limit ?? 200
  const offset = cursor ? Number(cursor) : 0
  const rows = matched.slice(offset, offset + lim)
  const next_cursor = offset + lim < matched.length ? String(offset + lim) : undefined
  return { rows, matched_count: matched.length, elapsed_ms: 3, next_cursor }
}

// Roll up one trace's spans into a `TraceSummary`-shaped object: the representative span is the
// root (`parent_span_id == null`) if present, else the earliest-start span; `duration_ns` falls
// back to `max(end) - min(start)` across all spans when the representative has no duration.
function traceSummary(trace: MockTrace): TraceSummary {
  const spans = trace.spans
  const root =
    spans.find((s) => s.parent_span_id == null) ??
    spans.reduce((a, b) => (BigInt(a.start_time_nanos) <= BigInt(b.start_time_nanos) ? a : b))
  const services = [...new Set(spans.map((s) => s.service))].sort()
  const error_count = spans.filter((s) => s.status_code === 2).length

  let duration_ns = root.duration_nanos != null ? BigInt(root.duration_nanos) : null
  if (duration_ns == null) {
    const ends = spans.filter((s) => s.end_time_nanos != null).map((s) => BigInt(s.end_time_nanos))
    if (ends.length) {
      const starts = spans.map((s) => BigInt(s.start_time_nanos))
      const minStart = starts.reduce((a, b) => (a < b ? a : b))
      const maxEnd = ends.reduce((a, b) => (a > b ? a : b))
      duration_ns = maxEnd - minStart
    }
  }

  return {
    trace_id: trace.trace_id,
    root_service: root.service ?? null,
    root_name: root.name ?? null,
    start_ts: root.start_time_nanos,
    duration_ns: duration_ns == null ? null : duration_ns.toString(),
    span_count: spans.length,
    error_count,
    services,
  }
}

function sortTraces(list: TraceSummary[], sort?: string): TraceSummary[] {
  const arr = [...list]
  if (sort === 'slowest') {
    arr.sort((a, b) => {
      if (a.duration_ns == null) return 1
      if (b.duration_ns == null) return -1
      return cmpBigDesc(BigInt(a.duration_ns), BigInt(b.duration_ns))
    })
  } else if (sort === 'errors') {
    arr.sort((a, b) => b.error_count - a.error_count || cmpBigDesc(BigInt(a.start_ts), BigInt(b.start_ts)))
  } else {
    arr.sort((a, b) => cmpBigDesc(BigInt(a.start_ts), BigInt(b.start_ts)))
  }
  return arr
}

// mockSearchTraces: a trace matches if at least one of its spans matches the grammar query (mirrors
// the backend's `search_traces`), but the rollup (span_count/error_count/services/representative)
// is computed over ALL of that trace's spans, not just the matching ones. Returns the envelope
// matching POST /api/traces/search: { traces, matched_count, elapsed_ms, next_cursor? }.
export function mockSearchTraces({ query, sort, limit, cursor }: TraceSearchRequest = {}): TraceSearchEnvelope {
  const matchedSpans = filterSpans({ query })
  const traceIds = new Set(matchedSpans.map((s) => s.trace_id))
  const summaries = sortTraces(
    TRACES.filter((t) => traceIds.has(t.trace_id)).map(traceSummary),
    sort,
  )
  const lim = limit ?? 100
  const offset = cursor ? Number(cursor) : 0
  const traces = summaries.slice(offset, offset + lim)
  const next_cursor = offset + lim < summaries.length ? String(offset + lim) : undefined
  return { traces, matched_count: summaries.length, elapsed_ms: 3, next_cursor }
}

// Fixed + promoted + the distinct attribute keys present in the spans corpus.
export function mockTracesFields(): FieldEntry[] {
  const FIXED = [
    'trace_id',
    'span_id',
    'parent_span_id',
    'name',
    'kind',
    'start_time_nanos',
    'end_time_nanos',
    'duration_nanos',
    'status_code',
    'scope_name',
  ]
  const PROMOTED = ['service.name']
  const attrKeys = new Set<string>()
  for (const s of TRACE_SPANS) {
    for (const k of Object.keys(s.attributes ?? {})) attrKeys.add(k)
  }
  return [
    ...FIXED.map((name): FieldEntry => ({ name, kind: 'fixed' })),
    ...PROMOTED.map((name): FieldEntry => ({ name, kind: 'promoted' })),
    ...[...attrKeys].sort().map((name): FieldEntry => ({ name, kind: 'attribute' })),
  ]
}

// Top values + counts for one span field over the filtered corpus, sorted by count desc.
export function mockTracesFacet(field: string, query: string, start: string, end: string, limit = 50): FacetResult {
  const matched = filterSpans({ query })
  const counts = new Map<string, number>()
  for (const s of matched) {
    const value =
      field === 'service' || field === 'service.name'
        ? s.service
        : field === 'status_text' || field === 'kind_text'
          ? s[field]
          : s.attributes?.[field]
    if (value == null) continue
    counts.set(value, (counts.get(value) ?? 0) + 1)
  }
  const values = [...counts.entries()]
    .map(([value, count]) => ({ value, count }))
    .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value))
  return { values: values.slice(0, limit), capped: values.length > limit }
}

// Status-stacked span-volume histogram over [start, end] (nanos strings) with `buckets` equal-
// width bins, mirroring `mockHistogram`'s bucketing but keyed by OTLP status (ok/error/unset).
export function mockTracesHistogram(query: string, start: string, end: string, buckets = 48): TraceHistogramBucket[] {
  const NS = 1_000_000n
  const startMs = Number(BigInt(start) / NS)
  const endMs = Number(BigInt(end) / NS)
  const n = Math.max(1, buckets)
  const width = Math.max(1, (endMs - startMs) / n)
  const out: TraceHistogramBucket[] = Array.from({ length: n }, (_, i) => ({
    t: (BigInt(Math.round(startMs + i * width)) * NS).toString(),
    ok: 0,
    error: 0,
    unset: 0,
    total: 0,
  }))
  const matched = filterSpans({ query })
  for (const s of matched) {
    const ms = Number(BigInt(s.start_time_nanos) / NS)
    let i = Math.floor((ms - startMs) / width)
    if (i < 0) i = 0
    if (i >= n) i = n - 1
    out[i][statusKeyword(s.status_code)]++
    out[i].total++
  }
  return out
}

// Duration-distribution histogram + percentiles over the matched span set. Percentiles are read
// off the sorted duration array (index = floor(p * length)), which is monotone in `p` by
// construction. Linear buckets span [0, max duration] — a v1 simplification matching the
// backend's documented choice (see plan Task 8).
export function mockTracesLatency(query: string, start: string, end: string, buckets = 48): LatencyResult {
  const n = Math.max(1, buckets)
  const durations = filterSpans({ query })
    .map((s) => s.duration_nanos)
    .filter((d) => d != null)
    .sort((a, b) => a - b)

  if (durations.length === 0) {
    return {
      buckets: Array.from({ length: n }, () => ({ bucket_ns: '0', count: 0 })),
      p50: '0',
      p90: '0',
      p99: '0',
    }
  }

  const percentile = (p: number) => durations[Math.min(durations.length - 1, Math.floor(p * durations.length))]
  const max = durations[durations.length - 1]
  const width = Math.max(1, max / n)
  const out = Array.from({ length: n }, (_, i) => ({ bucket_ns: String(Math.round(i * width)), count: 0 }))
  for (const d of durations) {
    const i = Math.min(n - 1, Math.floor(d / width))
    out[i].count++
  }

  return {
    buckets: out,
    p50: String(percentile(0.5)),
    p90: String(percentile(0.9)),
    p99: String(percentile(0.99)),
  }
}

// RED metrics mock: aggregates the SAME filtered span corpus (`filterSpans`) the other trace
// mocks (`mockTracesLatency` et al.) read from, so grammar filtering works here too and the RED
// table stays consistent with the latency/histogram mocks. `group=service` rolls operations up
// per service (operation → null); `group=operation` (default) keys by (service, span name).
// Percentiles use the same sorted-index idiom as `mockTracesLatency` and are decimal-nanosecond
// strings, matching the real `/api/red` shape.
export function mockRed(query: string, start: string, end: string, group = 'operation'): RedRow[] {
  const groups = new Map<string, RedGroup>()
  for (const s of filterSpans({ query })) {
    const key = group === 'service' ? s.service : `${s.service} ${s.name}`
    let g = groups.get(key)
    if (!g) {
      g = { service: s.service, operation: group === 'service' ? null : s.name, count: 0, error_count: 0, durations: [] }
      groups.set(key, g)
    }
    g.count++
    if (s.status_code === 2) g.error_count++
    if (s.duration_nanos != null) g.durations.push(s.duration_nanos)
  }

  const percentile = (sorted: number[], p: number) =>
    sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] : 0
  const windowSecs = Math.max(1, Number(BigInt(end) - BigInt(start)) / 1e9)

  return [...groups.values()]
    .map((g) => {
      const sorted = [...g.durations].sort((a, b) => a - b)
      return {
        service: g.service,
        operation: g.operation,
        count: g.count,
        rate: g.count / windowSecs,
        error_count: g.error_count,
        error_rate: g.count ? g.error_count / g.count : 0,
        p50: String(percentile(sorted, 0.5)),
        p90: String(percentile(sorted, 0.9)),
        p99: String(percentile(sorted, 0.99)),
      }
    })
    .sort((a, b) => b.error_rate - a.error_rate)
}

// Per-service request-rate / latency / error timeseries for the overview dashboard (Task 13) and
// ServiceDetailView's charts in demo mode. Mirrors the real `GET /api/services/:svc/timeseries`
// bucket shape ServiceDetailView consumes: `ts` is an epoch-MILLISECOND Number (chart x-axis unit),
// `error_rate` a 0..1 fraction, and the percentiles are decimal-NANOSECOND strings (charts read
// them via `Number(p)/1e6`). Deterministic (seeded per service) so fixtures are stable across
// renders/tests. `start`/`end` (ns strings, optional) span the buckets across the selected window
// so the series lines up with the chart's start/end; absent, it defaults to the last 30 minutes.
export function mockServiceTimeseries(
  service: string,
  buckets = 48,
  { start, end }: { start?: string; end?: string } = {},
): ServiceTimeseriesPoint[] {
  const NS = 1_000_000n
  const endMs = end != null ? Number(BigInt(end) / NS) : Date.now()
  const startMs = start != null ? Number(BigInt(start) / NS) : endMs - 30 * 60 * 1000
  const n = Math.max(1, Number(buckets) | 0)
  const width = (endMs - startMs) / n
  const rnd = seeded(hashSeed('ts:' + (service ?? '')))
  const baseRate = 20 + (hashSeed(service ?? '') % 80) // per-service steady req/s
  return Array.from({ length: n }, (_, i) => {
    const ts = Math.round(startMs + i * width)
    const rate = Math.max(0, baseRate * (0.7 + (Math.sin(i / 6) + 1) * 0.25 + (rnd() - 0.5) * 0.2))
    const error_rate = Math.max(0, Math.min(0.2, 0.02 + Math.sin(i / 9) * 0.02 + (rnd() - 0.5) * 0.01))
    const p50ns = Math.round((40 + Math.sin(i / 7) * 15 + rnd() * 10) * 1e6)
    const p90ns = Math.round(p50ns * (2.4 + rnd() * 0.4))
    const p99ns = Math.round(p50ns * (4.5 + rnd() * 1.2))
    return {
      ts,
      rate: Math.round(rate * 10) / 10,
      error_rate: Math.round(error_rate * 1000) / 1000,
      p50: String(p50ns),
      p90: String(p90ns),
      p99: String(p99ns),
    }
  })
}

// ---- Metrics fixtures (Phase 3) -----------------------------------------------------------
// A small fixed metric corpus, mirroring the real /api/metrics/* response shapes (ns as strings).
const METRIC_DEFS: MetricDef[] = [
  { name: 'http.server.duration', type: 'histogram', unit: 'ms', temporality: 'cumulative', is_monotonic: null, keys: ['service', 'http.route', 'http.method', 'http.status_code'] },
  { name: 'http.server.requests', type: 'sum', unit: '1', temporality: 'cumulative', is_monotonic: true, keys: ['service', 'http.route', 'http.method', 'http.status_code'] },
  { name: 'rpc.server.duration', type: 'histogram', unit: 'ms', temporality: 'cumulative', is_monotonic: null, keys: ['service', 'rpc.method'] },
  { name: 'process.cpu.utilization', type: 'gauge', unit: '1', temporality: null, is_monotonic: null, keys: ['service', 'host.name'] },
  { name: 'process.memory.usage', type: 'gauge', unit: 'By', temporality: null, is_monotonic: null, keys: ['service', 'host.name'] },
  { name: 'system.disk.io', type: 'sum', unit: 'By', temporality: 'cumulative', is_monotonic: true, keys: ['service', 'device'] },
  { name: 'queue.depth', type: 'gauge', unit: '1', temporality: null, is_monotonic: null, keys: ['service', 'queue'] },
  { name: 'db.client.connections.usage', type: 'sum', unit: '1', temporality: 'cumulative', is_monotonic: false, keys: ['service', 'pool.name'] },
  { name: 'rpc.client.duration', type: 'exp_histogram', unit: 'ms', temporality: 'delta', is_monotonic: null, keys: ['service', 'rpc.method'] },
  { name: 'http.server.active_requests', type: 'summary', unit: '1', temporality: null, is_monotonic: null, keys: ['service'] },
]
const METRIC_DEFAULT_AGG: Record<string, string> = { gauge: 'avg', histogram: 'p99', exp_histogram: 'p99', summary: 'median' }
function metricDefaultAgg(def: MetricDef): string {
  if (def.type === 'sum') return def.is_monotonic ? 'rate' : 'sum'
  return METRIC_DEFAULT_AGG[def.type] ?? 'avg'
}
// Deterministic PRNG (no Date.now/Math.random — keeps fixtures stable across renders/tests).
function seeded(seed: number): () => number {
  let s = seed >>> 0
  return () => { s = (s * 1664525 + 1013904223) >>> 0; return s / 4294967296 }
}
function hashSeed(str: string): number {
  let h = 2166136261 >>> 0
  for (let i = 0; i < str.length; i++) h = Math.imul(h ^ str.charCodeAt(i), 16777619) >>> 0
  return h
}

export function mockMetricCatalog(
  startNs: string,
  endNs: string,
  { search, type }: { search?: string; type?: string } = {},
): MetricCatalogEntry[] {
  const lastSeen = String(BigInt(endNs) - 4_000_000_000n) // ~4s before window end
  let defs = METRIC_DEFS
  if (search) defs = defs.filter((d) => d.name.includes(search))
  if (type) defs = defs.filter((d) => d.type === type)
  return defs.map((d) => ({
    name: d.name, type: d.type, unit: d.unit,
    temporality: d.temporality, is_monotonic: d.is_monotonic,
    series_count: 3 + (hashSeed(d.name) % 1200),
    last_seen: lastSeen,
  }))
}

export function mockMetricMetadata(name: string, startNs: string, endNs: string): MetricMetadata | null {
  const d = METRIC_DEFS.find((m) => m.name === name)
  if (!d) return null
  return {
    name: d.name, type: d.type, temporality: d.temporality, is_monotonic: d.is_monotonic,
    unit: d.unit, series_count: 3 + (hashSeed(d.name) % 1200),
    last_seen: String(BigInt(endNs) - 4_000_000_000n),
    attribute_keys: d.keys.slice(),
  }
}

export function mockMetricLabels(metric: string, key: string | null, startNs: string, endNs: string): MetricLabelsResult {
  const d = METRIC_DEFS.find((m) => m.name === metric)
  const keys = d ? d.keys : ['service']
  if (!key) return { keys: keys.slice() }
  if (key === 'service' || key === 'service.name') return { values: SERVICES.slice(), capped: false }
  // synthetic per-key values
  const vals = Array.from({ length: 4 }, (_, i) => `${key}-${i + 1}`)
  return { values: vals, capped: false }
}

// ---- Data & Retention (Settings) fixtures --------------------------------------------------
// Per-signal storage stats (file/row counts, ts range, on-disk bytes), mirroring GET /api/storage.
// Nested under `signals` + a top-level `durable` replication-status block (Task 7 reshape).
export const mockStorage = {
  signals: {
    logs:   { file_count: 12, total_rows: 4_200_000, min_ts_nanos: 1_749_000_000_000_000_000, max_ts_nanos: 1_751_000_000_000_000_000, bytes: 210_000_000, durable_bytes: 176_000_000 },
    traces: { file_count: 5,  total_rows: 900_000,   min_ts_nanos: 1_749_500_000_000_000_000, max_ts_nanos: 1_751_000_000_000_000_000, bytes: 64_000_000,  durable_bytes: 38_000_000 },
    metrics:{ file_count: 3,  total_rows: 300_000,   min_ts_nanos: 1_750_000_000_000_000_000, max_ts_nanos: 1_751_000_000_000_000_000, bytes: 12_000_000,  durable_bytes: 0 },
    uptime: { monitor_count: 4, heartbeat_count: 51_200, incident_count: 3, oldest_heartbeat_ts: 1_749_000_000_000, newest_heartbeat_ts: 1_751_000_000_000 },
  },
  durable: { configured: true, pending: 3, last_replicated_ms: 1_751_000_000_000 },
}
// Per-signal retention in days, mirroring GET/PUT /api/retention. `let`, not `const`: api.js's
// setRetention() mock fallback mutates it in place so repeated calls in mock mode are consistent.
export let mockRetention = { logs: 30, traces: 7, metrics: 90, uptime: 30 }

// Synthetic usage series: a sawtooth footprint that dips at retention boundaries + noisy ingestion.
const USAGE_WINDOWS: Record<string, [number, number]> = { '1h': [3_600_000, 60_000], '24h': [86_400_000, 300_000], '7d': [604_800_000, 1_800_000], '30d': [2_592_000_000, 7_200_000] }
export function mockUsageSeries(window = '24h'): UsageSeriesResponse {
  const [span, bucket_ms] = USAGE_WINDOWS[window] ?? USAGE_WINDOWS['24h']
  const now = 1_751_000_000_000
  const n = Math.floor(span / bucket_ms)
  const bases: Record<string, number> = { logs: 210_000_000, traces: 64_000_000, metrics: 12_000_000 }
  const rates: Record<string, number> = { logs: 1200, traces: 300, metrics: 90 }
  const series: Record<string, UsagePoint[]> = {}
  for (const sig of ['logs', 'traces', 'metrics']) {
    series[sig] = Array.from({ length: n }, (_, i) => {
      const ts = now - (n - 1 - i) * bucket_ms
      const saw = ((i % Math.max(8, Math.floor(n / 4))) / n)           // rises then resets
      const hot = Math.round(bases[sig] * (0.6 + saw))
      const ingest_rows = i === 0 ? null : Math.round(rates[sig] * (bucket_ms / 1000) * (0.7 + (Math.sin(i / 5) + 1) * 0.3))
      return { ts, hot_bytes: hot, durable_bytes: Math.round(hot * 0.84), total_rows: Math.round(hot / 50),
               ingest_rows, ingest_bytes: ingest_rows == null ? null : ingest_rows * 50 }
    })
  }
  return { window, bucket_ms, series }
}

// ---- Uptime monitors fixture ---------------------------------------------------------------
// Relocated out of api.js so the overview dashboard (Task 13) and the uptime demo share ONE
// fixture. Mirrors GET /api/monitors rows; `api.listMonitors`/`getMonitor` fall back to this.
export const mockMonitors = [
  {
    id: 'mock-1',
    name: 'Example (mock)',
    type: 'http',
    target: 'https://example.com',
    interval_secs: 60,
    timeout_secs: 10,
    retries: 2,
    enabled: true,
    last_state: 'up',
    last_check_at: Date.now(),
    last_latency_ms: 42,
  },
]

export function mockMetricQuery(request: MetricQueryRequest): MetricQueryResponse {
  const NS = 1_000_000n
  const q = request.queries[0]
  const def = METRIC_DEFS.find((m) => m.name === q.metric)
  const startMs = Number(BigInt(request.start) / NS)
  const endMs = Number(BigInt(request.end) / NS)
  const n = 200
  const width = Math.max(1, (endMs - startMs) / n)
  const groupKey = (q.group_by && q.group_by[0]) || null
  const defaultAgg = def ? metricDefaultAgg(def) : 'avg'
  // Unknown metric → empty series set (still 200), matching the backend.
  if (!def) {
    return {
      results: [{ id: q.id, series: [], default_agg: defaultAgg }],
      step: (BigInt(Math.round(width)) * NS).toString(),
      capped: false,
      elapsed_ms: 1,
    }
  }
  const groupValues = groupKey
    ? (groupKey === 'service' ? SERVICES.slice(0, 3) : [`${groupKey}-1`, `${groupKey}-2`])
    : [null]
  const series = groupValues.map((gv, gi): MetricSeries => {
    const rnd = seeded(hashSeed(def.name + ':' + (gv ?? '')))
    const base = 40 + gi * 30
    const points = Array.from({ length: n }, (_, i) => {
      const t = (BigInt(Math.round(startMs + i * width)) * NS).toString()
      // gentle sine + noise; occasional null gap
      const v = rnd() < 0.02 ? null : Math.max(0, base + Math.sin(i / 9 + gi) * base * 0.5 + (rnd() - 0.5) * base * 0.4)
      return { t, v: v === null ? null : Math.round(v * 100) / 100 }
    })
    const labels = groupKey ? { [groupKey]: gv } : {}
    return { labels, points, exemplars: [] }
  })
  return {
    results: [{ id: q.id, series, default_agg: defaultAgg }],
    step: (BigInt(Math.round(width)) * NS).toString(),
    capped: false,
    elapsed_ms: 2,
  }
}

// ---- RUM (Real User Monitoring) fixtures ---------------------------------------------------
// A fixed two-app demo corpus mirroring GET /api/rum/*. Thresholds (good_max/poor_min) mirror the
// backend's Core Web Vitals table (`crates/photon-core/src/rum.rs::thresholds()`); the client
// never hardcodes rating cutoffs — every vitals row carries its own from the response, same as
// the real API.
let mockApps: RumApp[] = [
  { name: 'web-storefront', key: 'pk_live_mockweb', allowed_origins: ['https://shop.example.com'], sample_rate: 1.0, rate_limit: 5000, created_at: 1_700_000_000_000 },
  { name: 'admin-dashboard', key: 'pk_live_mockadmin', allowed_origins: ['https://admin.example.com'], sample_rate: 1.0, rate_limit: 5000, created_at: 1_700_000_000_000 },
]

export function mockRumApps(): { apps: RumApp[] } {
  return { apps: [...mockApps].sort((a, b) => a.name.localeCompare(b.name)) }
}

export function mockCreateRumApp(name: string, input: { allowed_origins: string[]; sample_rate?: number; rate_limit?: number }): { ok: boolean; error?: string; key?: string } {
  const trimmed = name.trim()
  if (!trimmed) return { ok: false, error: 'name must not be empty' }
  if (!input.allowed_origins.length) return { ok: false, error: 'at least one allowed origin is required' }
  if (mockApps.some((a) => a.name === trimmed)) return { ok: false, error: 'an app with that name already exists' }
  const key = `pk_live_mock${Math.random().toString(16).slice(2, 12)}`
  mockApps = [...mockApps, { name: trimmed, key, allowed_origins: input.allowed_origins, sample_rate: input.sample_rate ?? 1.0, rate_limit: input.rate_limit ?? 5000, created_at: Date.now() }]
  return { ok: true, key }
}

export function mockUpdateRumApp(name: string, input: { allowed_origins?: string[]; sample_rate?: number; rate_limit?: number }): { ok: boolean; error?: string } {
  const app = mockApps.find((a) => a.name === name)
  if (!app) return { ok: false, error: 'no such app' }
  mockApps = mockApps.map((a) => (a.name === name ? { ...a, allowed_origins: input.allowed_origins ?? a.allowed_origins, sample_rate: input.sample_rate ?? a.sample_rate, rate_limit: input.rate_limit ?? a.rate_limit } : a))
  return { ok: true }
}

export function mockRotateRumAppKey(name: string): { ok: boolean; error?: string; key?: string } {
  const app = mockApps.find((a) => a.name === name)
  if (!app) return { ok: false, error: 'no such app' }
  const key = `pk_live_mock${Math.random().toString(16).slice(2, 12)}`
  mockApps = mockApps.map((a) => (a.name === name ? { ...a, key } : a))
  return { ok: true, key }
}

export function mockDeleteRumApp(name: string): { ok: boolean; error?: string } {
  if (!mockApps.some((a) => a.name === name)) return { ok: false, error: 'no such app' }
  mockApps = mockApps.filter((a) => a.name !== name)
  return { ok: true }
}

const RUM_THRESHOLDS: Record<string, [number, number]> = {
  'web_vitals.lcp': [2500.0, 4000.0],
  'web_vitals.inp': [200.0, 500.0],
  'web_vitals.cls': [0.1, 0.25],
  'web_vitals.fcp': [1800.0, 3000.0],
  'web_vitals.ttfb': [800.0, 1800.0],
}

function rumRating(p75: number, goodMax: number, poorMin: number): 'good' | 'needs' | 'poor' {
  if (p75 <= goodMax) return 'good'
  if (p75 <= poorMin) return 'needs'
  return 'poor'
}

function rumVitalRow(
  metric: string,
  p75: number,
  dist: { good: number; needs: number; poor: number },
): RumVitalRow {
  const [good_max, poor_min] = RUM_THRESHOLDS[metric]
  return {
    metric,
    p75,
    rating: rumRating(p75, good_max, poor_min),
    good_max,
    poor_min,
    dist: { ...dist, total: dist.good + dist.needs + dist.poor },
  }
}

// The approved mockup's "web-storefront" story: LCP 2.8s needs-work, INP 184ms good, CLS 0.06
// good, FCP 1.6s good, TTFB 620ms good. `admin-dashboard` is a healthier second app so switching
// apps in the UI shows visibly different data.
const RUM_VITALS_BY_APP: Record<string, RumVitalRow[]> = {
  'web-storefront': [
    rumVitalRow('web_vitals.lcp', 2800.0, { good: 58, needs: 31, poor: 11 }),
    rumVitalRow('web_vitals.inp', 184.0, { good: 84, needs: 12, poor: 4 }),
    rumVitalRow('web_vitals.cls', 0.06, { good: 88, needs: 9, poor: 3 }),
    rumVitalRow('web_vitals.fcp', 1600.0, { good: 79, needs: 16, poor: 5 }),
    rumVitalRow('web_vitals.ttfb', 620.0, { good: 81, needs: 14, poor: 5 }),
  ],
  'admin-dashboard': [
    rumVitalRow('web_vitals.lcp', 1900.0, { good: 91, needs: 7, poor: 2 }),
    rumVitalRow('web_vitals.inp', 120.0, { good: 95, needs: 4, poor: 1 }),
    rumVitalRow('web_vitals.cls', 0.03, { good: 96, needs: 3, poor: 1 }),
    rumVitalRow('web_vitals.fcp', 1100.0, { good: 93, needs: 5, poor: 2 }),
    rumVitalRow('web_vitals.ttfb', 340.0, { good: 97, needs: 2, poor: 1 }),
  ],
}

export function mockRumVitals(app: string): { app: string; vitals: RumVitalRow[] } {
  return { app, vitals: RUM_VITALS_BY_APP[app] ?? RUM_VITALS_BY_APP['web-storefront'] }
}

// Site-wide device-type split for web-storefront — mobile is the worst bucket (LCP 5.6s poor),
// matching the "mobile worst" story. Reused verbatim as `/checkout`'s own breakdown below since
// checkout is where that mobile traffic concentrates.
const RUM_DEVICE_BREAKDOWN: RumBreakdownRow[] = [
  { key: 'mobile', pageviews: 77000, lcp_p75: 5600.0, inp_p75: 240.0, cls_p75: 0.11 },
  { key: 'desktop', pageviews: 96000, lcp_p75: 2100.0, inp_p75: 140.0, cls_p75: 0.04 },
  { key: 'tablet', pageviews: 31000, lcp_p75: 3200.0, inp_p75: 190.0, cls_p75: 0.08 },
]

// Per-page rollup (route → aggregate). `/checkout` is deliberately the worst page (LCP 4.3s
// poor), matching the contract's "web-storefront" demo story.
const RUM_PAGES: RumPageRow[] = [
  { route: '/checkout', pageviews: 142000, lcp_p75: 4300.0, inp_p75: 210.0, cls_p75: 0.09 },
  { route: '/product/:id', pageviews: 268000, lcp_p75: 2600.0, inp_p75: 175.0, cls_p75: 0.05 },
  { route: '/cart', pageviews: 98000, lcp_p75: 2300.0, inp_p75: 160.0, cls_p75: 0.04 },
  { route: '/search', pageviews: 61000, lcp_p75: 2050.0, inp_p75: 150.0, cls_p75: 0.03 },
  { route: '/home', pageviews: 210000, lcp_p75: 1700.0, inp_p75: 120.0, cls_p75: 0.02 },
]

const RUM_BREAKDOWN_BY_DIMENSION: Record<string, RumBreakdownRow[]> = {
  'device.type': RUM_DEVICE_BREAKDOWN,
  'browser.route': RUM_PAGES.map((p) => ({
    key: p.route,
    pageviews: p.pageviews,
    lcp_p75: p.lcp_p75,
    inp_p75: p.inp_p75,
    cls_p75: p.cls_p75,
  })),
  'browser.name': [
    { key: 'Chrome', pageviews: 420000, lcp_p75: 2700.0, inp_p75: 180.0, cls_p75: 0.06 },
    { key: 'Safari', pageviews: 160000, lcp_p75: 3100.0, inp_p75: 210.0, cls_p75: 0.08 },
    { key: 'Firefox', pageviews: 38000, lcp_p75: 2500.0, inp_p75: 170.0, cls_p75: 0.05 },
    { key: 'Edge', pageviews: 21000, lcp_p75: 2400.0, inp_p75: 165.0, cls_p75: 0.05 },
  ],
  'geo.country': [
    { key: 'US', pageviews: 340000, lcp_p75: 2400.0, inp_p75: 170.0, cls_p75: 0.05 },
    { key: 'DE', pageviews: 88000, lcp_p75: 2900.0, inp_p75: 190.0, cls_p75: 0.06 },
    { key: 'IN', pageviews: 72000, lcp_p75: 3900.0, inp_p75: 230.0, cls_p75: 0.09 },
    { key: 'BR', pageviews: 61000, lcp_p75: 3400.0, inp_p75: 200.0, cls_p75: 0.07 },
    { key: 'GB', pageviews: 40000, lcp_p75: 2600.0, inp_p75: 175.0, cls_p75: 0.05 },
  ],
  'network.connection': [
    { key: '4g', pageviews: 460000, lcp_p75: 2700.0, inp_p75: 180.0, cls_p75: 0.06 },
    { key: 'wifi', pageviews: 210000, lcp_p75: 2000.0, inp_p75: 140.0, cls_p75: 0.04 },
    { key: '3g', pageviews: 68000, lcp_p75: 5200.0, inp_p75: 260.0, cls_p75: 0.12 },
    { key: 'slow-2g', pageviews: 9000, lcp_p75: 7400.0, inp_p75: 340.0, cls_p75: 0.18 },
  ],
}

export function mockRumBreakdown(
  app: string,
  dimension: string,
): { app: string; dimension: string; rows: RumBreakdownRow[] } {
  return { app, dimension, rows: RUM_BREAKDOWN_BY_DIMENSION[dimension] ?? [] }
}

// Per-app page rollups. `web-storefront` keeps the full contract story (RUM_PAGES); `admin-dashboard`
// is the healthy, lower-traffic second app so the `/rum` executive summary reads as a real fleet.
const RUM_PAGES_BY_APP: Record<string, RumPageRow[]> = {
  'web-storefront': RUM_PAGES,
  'admin-dashboard': [
    { route: '/dashboard', pageviews: 18000, lcp_p75: 1800.0, inp_p75: 110.0, cls_p75: 0.02 },
    { route: '/reports', pageviews: 9000, lcp_p75: 2200.0, inp_p75: 130.0, cls_p75: 0.03 },
    { route: '/settings', pageviews: 4000, lcp_p75: 1600.0, inp_p75: 100.0, cls_p75: 0.01 },
  ],
}

export function mockRumPages(app: string): { app: string; pages: RumPageRow[] } {
  return { app, pages: RUM_PAGES_BY_APP[app] ?? RUM_PAGES }
}

// Issue list: `/checkout`'s TypeError is the headline issue from the contract's demo story
// ("Cannot read properties of undefined (reading 'price')"). `route` is mock-only bookkeeping
// (not part of the API shape) so `/pages/detail` can scope errors to a route.
const RUM_ERRORS: RumErrorRow[] = [
  {
    fingerprint: 'rum-chk-price-undef', exception_type: 'TypeError',
    message: "Cannot read properties of undefined (reading 'price')",
    count: 3420, sessions: 1890, route: '/checkout',
  },
  {
    fingerprint: 'rum-cart-qty-range', exception_type: 'RangeError',
    message: 'Invalid array length', count: 980, sessions: 640, route: '/cart',
  },
  {
    fingerprint: 'rum-search-timeout', exception_type: 'Error',
    message: 'Network request failed: search timed out after 8000ms',
    count: 540, sessions: 410, route: '/search',
  },
  {
    fingerprint: 'rum-home-hydrate-null', exception_type: 'TypeError',
    message: "Cannot read properties of null (reading 'addEventListener')",
    count: 210, sessions: 180, route: '/home',
  },
]

function publicRumError({ fingerprint, exception_type, message, count, sessions }: RumErrorRow): RumPublicError {
  return { fingerprint, exception_type, message, count, sessions }
}

// Per-app error issues. `web-storefront` carries the noisy contract story; `admin-dashboard` is the
// healthy app with a single minor issue, so the fleet "Live issues" feed ranks storefront first.
const RUM_ERRORS_BY_APP: Record<string, RumErrorRow[]> = {
  'web-storefront': RUM_ERRORS,
  'admin-dashboard': [
    {
      fingerprint: 'rum-admin-report-null', exception_type: 'TypeError',
      message: "Cannot read properties of null (reading 'rows')",
      count: 34, sessions: 12, route: '/reports',
    },
  ],
}

// `_q` is accepted (mirroring the real API's search param) but is a no-op here — the mock
// fixture is small and fixed, so filtering it by query would add complexity for little payoff.
export function mockRumErrors(app: string, _q?: string): { app: string; errors: RumPublicError[] } {
  return { app, errors: (RUM_ERRORS_BY_APP[app] ?? RUM_ERRORS).map(publicRumError) }
}

// Facet counts over the same per-app error fixture `mockRumErrors` reads from — six fields
// mirroring the real `/api/rum/errors/facets` shape (`exception.type`/`error.kind` derived from
// the fixture; the rest are plausible fixed splits since the mock corpus doesn't track them).
export function mockRumErrorFacets(app: string): RumErrorFacetsResult {
  const errs = mockRumErrors(app).errors
  const tally = (pick: (e: any) => string) => {
    const m = new Map<string, number>()
    for (const e of errs) m.set(pick(e), (m.get(pick(e)) ?? 0) + e.count)
    return [...m].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count)
  }
  return {
    app,
    facets: {
      'exception.type': { values: tally((e) => e.exception_type), capped: false },
      'error.kind': { values: [{ value: 'exception', count: errs.reduce((s, e) => s + e.count, 0) }], capped: false },
      'browser.route': { values: tally((e) => e.route ?? '/'), capped: false },
      'browser.name': { values: [{ value: 'Chrome', count: 120 }, { value: 'Safari', count: 40 }], capped: false },
      'device.type': { values: [{ value: 'mobile', count: 96 }, { value: 'desktop', count: 64 }], capped: false },
      'network.connection': { values: [{ value: '4g', count: 110 }, { value: 'wifi', count: 50 }], capped: false },
    },
  }
}

// LCP attribution demo numbers (approved mockup): the four sub-parts sum to /checkout's LCP p75
// (900 + 320 + 2100 + 980 = 4300ms, matching that page's `lcp_p75` above) so the segmented bar's
// total lines up with the vital card next to it. Matches the real `attribution.lcp` shape from
// `GET /api/rum/pages/detail` (`crates/photon-api/src/rum.rs::lcp_attribution_json`).
const CHECKOUT_LCP_ATTRIBUTION: RumLcpAttribution = {
  ttfb: 900,
  resource_load_delay: 320,
  resource_load_time: 2100,
  element_render_delay: 980,
  element: '<img class="hero-product">',
}
const NO_LCP_ATTRIBUTION: RumLcpAttribution = {
  ttfb: null,
  resource_load_delay: null,
  resource_load_time: null,
  element_render_delay: null,
  element: null,
}

export function mockRumPageDetail(app: string, route: string): RumPageDetail {
  const page = RUM_PAGES.find((p) => p.route === route)
  const vitals = page
    ? { pageviews: page.pageviews, lcp_p75: page.lcp_p75, inp_p75: page.inp_p75, cls_p75: page.cls_p75 }
    : null
  const breakdown = route === '/checkout' ? RUM_DEVICE_BREAKDOWN : []
  const errors = RUM_ERRORS.filter((e) => e.route === route).map(publicRumError)
  const attribution = { lcp: route === '/checkout' ? CHECKOUT_LCP_ATTRIBUTION : NO_LCP_ATTRIBUTION }
  return { app, route, vitals, breakdown, errors, attribution }
}

// Issue detail: scoped to `app` the same way `mockRumErrors` is (per-app fixture map, falling
// back to the storefront's `RUM_ERRORS`), then looked up by fingerprint within that app's list.
export function mockRumErrorDetail(app: string, fingerprint: string): RumErrorDetail {
  const list = RUM_ERRORS_BY_APP[app] ?? RUM_ERRORS
  const e = list.find((x) => x.fingerprint === fingerprint)
  const now = Date.now()
  // The real API (`crates/photon-api/src/rum.rs::error_detail_json`) returns first_seen/last_seen/
  // series[].t/events[].timestamp as raw epoch NANOSECONDS (a DataFusion `cast(timestamp, Int64)` /
  // histogram bucket-start over the ns window bounds — see `photon-query`'s `rum_error_detail`).
  // Build the fixture in ms (as before) then multiply by 1e6 so the offline fallback is shape- and
  // unit-identical to the real backend.
  const series: RumErrorCountBucket[] = Array.from({ length: 24 }, (_, i) => ({
    t: (now - (23 - i) * 3_600_000) * 1_000_000,
    count: e ? Math.max(0, Math.round((e.count / 24) * (0.5 + Math.sin(i) * 0.5 + 0.5))) : 0,
  }))
  const route = e?.route ?? '/'
  const occurrences = e?.count ?? 0
  return {
    app,
    fingerprint,
    exception_type: e?.exception_type ?? 'Error',
    message: e?.message ?? 'Unknown error',
    error_kind: 'exception',
    first_seen: (now - 6 * 3_600_000) * 1_000_000,
    last_seen: now * 1_000_000,
    occurrences,
    sessions: e?.sessions ?? 0,
    series,
    tags: [
      { field: 'browser.name', values: [{ value: 'Chrome', count: occurrences }] },
      {
        field: 'device.type',
        values: [
          { value: 'mobile', count: Math.round(occurrences * 0.6) },
          { value: 'desktop', count: Math.round(occurrences * 0.4) },
        ],
      },
      { field: 'browser.route', values: [{ value: route, count: occurrences }] },
      { field: 'network.connection', values: [{ value: '4g', count: occurrences }] },
    ],
    sample_stack: `${e?.exception_type ?? 'Error'}: ${e?.message ?? ''}\n    at handler (app.js:42:13)\n    at dispatch (vendor.js:118:9)`,
    events: Array.from({ length: Math.min(20, occurrences) }, (_, i) => ({
      timestamp: (now - i * 300_000) * 1_000_000,
      route,
      browser: 'Chrome',
      device: i % 2 ? 'mobile' : 'desktop',
      session: `sess-${1000 + i}`,
      trace_id: i % 3 === 0 ? null : 'a'.repeat(32),
    })),
  }
}

// ---- Infra (host/GPU resource monitoring) fixtures -----------------------------------------
// A small fixed host corpus mirroring GET /api/infra/hosts, /:host, and /:host/timeseries. Field
// names match the real API's camelCase JSON exactly (cpuUtil/memUtil/lastSeenNs/hasGpu/
// totalRamBytes/gpus) so api.ts's mock fallback is shape-identical to the server. The
// `InfraHost*`/`InfraSeriesResult` shapes themselves are canonically defined in `api.ts` (the
// source of truth for the wire contract) and imported type-only above — no duplicate declarations.

interface InfraHostFixture {
  host: string
  os: string
  cores: number
  totalRamBytes: number
  hasGpu: boolean
  gpus: string[]
  cpuUtil: number
  memUtil: number
}

// Two ordinary web hosts + one GPU node, so the host table and the GPU-only panel both have
// something real to show in demo mode.
const INFRA_HOSTS: InfraHostFixture[] = [
  { host: 'web-1', os: 'linux', cores: 8, totalRamBytes: 16 * 1024 ** 3, hasGpu: false, gpus: [], cpuUtil: 0.32, memUtil: 0.54 },
  { host: 'web-2', os: 'linux', cores: 8, totalRamBytes: 16 * 1024 ** 3, hasGpu: false, gpus: [], cpuUtil: 0.41, memUtil: 0.61 },
  { host: 'gpu-node-1', os: 'linux', cores: 32, totalRamBytes: 128 * 1024 ** 3, hasGpu: true, gpus: ['NVIDIA A100'], cpuUtil: 0.58, memUtil: 0.72 },
]

export function mockInfraHosts(): InfraHostsResult {
  const now = (BigInt(Date.now()) * MS).toString()
  return {
    hosts: INFRA_HOSTS.map((h) => ({
      host: h.host,
      cpuUtil: h.cpuUtil,
      memUtil: h.memUtil,
      lastSeenNs: now,
      hasGpu: h.hasGpu,
    })),
  }
}

export function mockInfraHost(host: string): InfraHostDetail {
  const h = INFRA_HOSTS.find((x) => x.host === host) ?? INFRA_HOSTS[0]
  return {
    host: h.host,
    os: h.os,
    cores: h.cores,
    totalRamBytes: h.totalRamBytes,
    gpus: h.gpus,
    lastSeenNs: (BigInt(Date.now()) * MS).toString(),
  }
}

// One series-group definition per curated resource panel, mirroring the backend's primary
// group-by attribute per resource (`InfraResource::primary` in photon-query/src/infra.rs): cpu
// groups by `cpu` (core index/total), memory by `host.name`, disk by `mountpoint`, network by
// `direction`, gpu by `gpu` index — only present when the host reports one.
function infraSeriesGroups(resource: string, h: InfraHostFixture): { labels: Record<string, string>; base: number }[] {
  switch (resource) {
    case 'cpu':
      return [{ labels: { cpu: 'total' }, base: h.cpuUtil }]
    case 'memory':
      return [{ labels: { 'host.name': h.host }, base: h.memUtil }]
    case 'disk':
      return [
        { labels: { mountpoint: '/' }, base: 0.42 },
        { labels: { mountpoint: '/data' }, base: 0.67 },
      ]
    case 'network':
      return [
        { labels: { direction: 'receive' }, base: 1_500_000 },
        { labels: { direction: 'transmit' }, base: 400_000 },
      ]
    case 'gpu':
      return h.hasGpu ? [{ labels: { gpu: '0' }, base: 0.63 }] : []
    default:
      return []
  }
}

// Synthetic per-resource timeseries over [start, end] (nanos strings), gently oscillating around
// each group's base value with deterministic per-label noise (same `seeded`/`hashSeed` idiom as
// `mockMetricQuery`) so repeated renders/tests are stable. Unlike some of the simpler per-app RUM
// mocks, this DOES honor the requested window (like `mockMetricQuery`) so `MetricChart` always has
// points inside the visible axis range regardless of which time range is selected.
export function mockInfraHostSeries(host: string, resource: string, startNs: string, endNs: string): InfraSeriesResult {
  const NS = 1_000_000n
  const startMs = Number(BigInt(startNs) / NS)
  const endMs = Number(BigInt(endNs) / NS)
  const n = 48
  const width = Math.max(1, (endMs - startMs) / n)
  const h = INFRA_HOSTS.find((x) => x.host === host) ?? INFRA_HOSTS[0]
  const groups = infraSeriesGroups(resource, h)
  const series: MetricSeries[] = groups.map(({ labels, base }) => {
    const rnd = seeded(hashSeed(`${host}:${resource}:${JSON.stringify(labels)}`))
    const points = Array.from({ length: n }, (_, i) => {
      const t = (BigInt(Math.round(startMs + i * width)) * NS).toString()
      const v = Math.max(0, base + Math.sin(i / 8) * base * 0.15 + (rnd() - 0.5) * base * 0.1)
      return { t, v: Math.round(v * 1000) / 1000 }
    })
    return { labels, points, exemplars: [] }
  })
  return { resource, series }
}

// ---- Alerts (webhook alert & notification engine) fixtures ---------------------------------
// Field names/shapes mirror GET/POST /api/alerts/* exactly (see api.ts's Alert* types, the
// canonical wire contract — imported type-only above, following the corrected infra-fixture
// convention rather than re-declaring the shapes here). A few rules/channels/incidents across all
// four signals so the Alerts view's rules table, channel grid, and incident history all have
// something real to render in demo mode.

let mockAlertChannelsData: AlertChannel[] = [
  {
    id: 'chan-1',
    name: 'Ops webhook bridge',
    kind: 'webhook',
    url: 'https://hooks.example.com/services/mock/webhook',
    secret: 'whsec_mock_1a2b3c',
    headers: null,
    created_at: 1_700_000_000_000,
    updated_at: 1_700_000_000_000,
  },
  {
    id: 'chan-2',
    name: 'PagerDuty inbound',
    kind: 'webhook',
    url: 'https://events.pagerduty.example.com/v2/enqueue',
    secret: null,
    headers: { Authorization: 'Bearer mock-token' },
    created_at: 1_700_000_500_000,
    updated_at: 1_700_000_500_000,
  },
]

let mockAlertRulesData: AlertRule[] = [
  {
    id: 'rule-1',
    name: 'High CPU (web fleet)',
    description: 'Sustained CPU pressure across web hosts',
    enabled: true,
    signal: 'metrics',
    condition: {
      signal: 'metrics',
      metric_name: 'system.cpu.utilization',
      label_filters: {},
      group_by: ['host.name'],
      agg: 'avg',
      window_secs: 300,
      cmp: 'gt',
      threshold: 0.9,
    },
    for_secs: 300,
    interval_secs: 60,
    severity: 'warning',
    channel_ids: ['chan-1'],
    created_at: 1_700_001_000_000,
    updated_at: 1_700_001_000_000,
  },
  {
    id: 'rule-2',
    name: 'Checkout error spike',
    description: null,
    enabled: true,
    signal: 'logs',
    condition: {
      signal: 'logs',
      query: 'severity:error service.name:checkout-api',
      group_by: null,
      window_secs: 600,
      cmp: 'gt',
      threshold: 100,
    },
    for_secs: 0,
    interval_secs: 60,
    severity: 'critical',
    channel_ids: ['chan-1', 'chan-2'],
    created_at: 1_700_002_000_000,
    updated_at: 1_700_002_000_000,
  },
  {
    id: 'rule-3',
    name: 'Storefront LCP regression',
    description: 'Largest Contentful Paint p75 above budget',
    enabled: false,
    signal: 'rum',
    condition: {
      signal: 'rum',
      app_id: 'web-storefront',
      route: null,
      kind: 'vital_lcp_p75',
      window_secs: 900,
      cmp: 'gt',
      threshold: 2500,
    },
    for_secs: 600,
    interval_secs: 60,
    severity: 'warning',
    channel_ids: ['chan-2'],
    created_at: 1_700_003_000_000,
    updated_at: 1_700_003_000_000,
  },
  {
    id: 'rule-4',
    name: 'checkout-api error rate',
    description: null,
    enabled: true,
    signal: 'traces',
    condition: {
      signal: 'traces',
      service: 'checkout-api',
      operation: null,
      kind: 'error_rate',
      window_secs: 300,
      cmp: 'gt',
      threshold: 5.0,
    },
    for_secs: 120,
    interval_secs: 60,
    severity: 'critical',
    channel_ids: ['chan-1'],
    created_at: 1_700_004_000_000,
    updated_at: 1_700_004_000_000,
  },
]

let mockAlertIncidentsData: AlertIncident[] = [
  {
    id: 1,
    rule_id: 'rule-1',
    series_key: 'host.name=web-2',
    started_at: Date.now() - 15 * 60_000,
    ended_at: null,
    peak_value: 0.94,
    severity: 'warning',
    summary: 'avg(system.cpu.utilization)=0.94 > 0.90',
  },
  {
    id: 2,
    rule_id: 'rule-2',
    series_key: '',
    started_at: Date.now() - 6 * 3_600_000,
    ended_at: Date.now() - 5 * 3_600_000,
    peak_value: 142,
    severity: 'critical',
    summary: 'count(severity:error service.name:checkout-api)=142 > 100',
  },
  {
    id: 3,
    rule_id: 'rule-4',
    series_key: '',
    started_at: Date.now() - 2 * 3_600_000,
    ended_at: Date.now() - 2 * 3_600_000 + 8 * 60_000,
    peak_value: 7.2,
    severity: 'critical',
    summary: 'error_rate(checkout-api)=7.2% > 5.0%',
  },
]

function compareCmp(cmp: string, value: number, threshold: number): boolean {
  switch (cmp) {
    case 'gt': return value > threshold
    case 'gte': return value >= threshold
    case 'lt': return value < threshold
    case 'lte': return value <= threshold
    default: return false
  }
}

// The label key a draft condition would group series by, if any — drives how many synthetic
// series `mockAlertPreview` fabricates (empty `group_by` → one aggregate series, matching the
// backend's documented "empty group_by → a single aggregate series (key = [])" rule).
function conditionGroupKey(condition: AlertCondition): string | null {
  if (condition.signal === 'metrics') return condition.group_by?.[0] ?? null
  if (condition.signal === 'logs') return condition.group_by ?? null
  return null
}

export function mockAlertRules(): AlertRule[] {
  return [...mockAlertRulesData]
}

export function mockAlertRule(id: string): AlertRule | undefined {
  return mockAlertRulesData.find((r) => r.id === id)
}

export function mockCreateAlertRule(input: AlertRuleInput): AlertRuleResult {
  const name = (input.name ?? '').trim()
  if (!name) return { ok: false, error: 'name must not be empty' }
  if (!input.signal || !input.condition) return { ok: false, error: 'signal and condition are required' }
  if (mockAlertRulesData.some((r) => r.name === name)) return { ok: false, error: 'a rule with that name already exists' }
  const now = Date.now()
  const rule: AlertRule = {
    id: `rule-mock${Math.random().toString(16).slice(2, 10)}`,
    name,
    description: input.description ?? null,
    enabled: input.enabled ?? true,
    signal: input.signal,
    condition: input.condition,
    for_secs: input.for_secs ?? 0,
    interval_secs: input.interval_secs ?? 60,
    severity: input.severity ?? 'warning',
    channel_ids: input.channel_ids ?? [],
    created_at: now,
    updated_at: now,
  }
  mockAlertRulesData = [...mockAlertRulesData, rule]
  return { ok: true, rule }
}

export function mockUpdateAlertRule(id: string, input: AlertRuleInput): AlertRuleResult {
  const existing = mockAlertRulesData.find((r) => r.id === id)
  if (!existing) return { ok: false, error: 'no such rule' }
  const updated: AlertRule = {
    ...existing,
    name: input.name ?? existing.name,
    description: input.description !== undefined ? input.description : existing.description,
    enabled: input.enabled ?? existing.enabled,
    signal: input.signal ?? existing.signal,
    condition: input.condition ?? existing.condition,
    for_secs: input.for_secs ?? existing.for_secs,
    interval_secs: input.interval_secs ?? existing.interval_secs,
    severity: input.severity ?? existing.severity,
    channel_ids: input.channel_ids ?? existing.channel_ids,
    updated_at: Date.now(),
  }
  mockAlertRulesData = mockAlertRulesData.map((r) => (r.id === id ? updated : r))
  return { ok: true, rule: updated }
}

export function mockDeleteAlertRule(id: string): MutationResult {
  if (!mockAlertRulesData.some((r) => r.id === id)) return { ok: false, error: 'no such rule' }
  mockAlertRulesData = mockAlertRulesData.filter((r) => r.id !== id)
  return { ok: true }
}

export function mockTestAlertRule(id: string): AlertTestRuleResult {
  const rule = mockAlertRule(id)
  if (!rule) return { ok: false, error: 'no such rule' }
  return { ok: true, series: mockAlertPreview(rule.condition).series }
}

// Dry-run a draft condition against a small deterministic corpus: 1-3 synthetic series hovering
// near the threshold (seeded on the condition JSON, so repeated renders of the same draft are
// stable), each flagged `breaching` per the same `cmp` the real evaluator would use.
export function mockAlertPreview(condition: AlertCondition): AlertPreviewResult {
  const rnd = seeded(hashSeed(JSON.stringify(condition)))
  const groupKey = conditionGroupKey(condition)
  const seriesKeys: Record<string, string>[] = groupKey
    ? [{ [groupKey]: `${groupKey}-1` }, { [groupKey]: `${groupKey}-2` }]
    : [{}]
  const threshold = condition.threshold
  const series: AlertPreviewSeries[] = seriesKeys.map((series_key) => {
    const noise = (rnd() - 0.5) * Math.max(Math.abs(threshold), 1) * 0.5
    const value = Math.round((threshold + noise) * 1000) / 1000
    return { series_key, value, breaching: compareCmp(condition.cmp, value, threshold) }
  })
  return { series }
}

export function mockAlertChannels(): AlertChannel[] {
  return [...mockAlertChannelsData]
}

export function mockAlertChannel(id: string): AlertChannel | undefined {
  return mockAlertChannelsData.find((c) => c.id === id)
}

export function mockCreateAlertChannel(input: AlertChannelInput): AlertChannelResult {
  const name = (input.name ?? '').trim()
  if (!name) return { ok: false, error: 'name must not be empty' }
  if (!input.url) return { ok: false, error: 'url is required' }
  if (mockAlertChannelsData.some((c) => c.name === name)) return { ok: false, error: 'a channel with that name already exists' }
  const now = Date.now()
  const channel: AlertChannel = {
    id: `chan-mock${Math.random().toString(16).slice(2, 10)}`,
    name,
    kind: input.kind ?? 'webhook',
    url: input.url,
    secret: input.secret ?? null,
    headers: input.headers ?? null,
    created_at: now,
    updated_at: now,
  }
  mockAlertChannelsData = [...mockAlertChannelsData, channel]
  return { ok: true, channel }
}

export function mockUpdateAlertChannel(id: string, input: AlertChannelInput): AlertChannelResult {
  const existing = mockAlertChannelsData.find((c) => c.id === id)
  if (!existing) return { ok: false, error: 'no such channel' }
  const updated: AlertChannel = {
    ...existing,
    name: input.name ?? existing.name,
    kind: input.kind ?? existing.kind,
    url: input.url ?? existing.url,
    secret: input.secret !== undefined ? input.secret : existing.secret,
    headers: input.headers !== undefined ? input.headers : existing.headers,
    updated_at: Date.now(),
  }
  mockAlertChannelsData = mockAlertChannelsData.map((c) => (c.id === id ? updated : c))
  return { ok: true, channel: updated }
}

export function mockDeleteAlertChannel(id: string): MutationResult {
  if (!mockAlertChannelsData.some((c) => c.id === id)) return { ok: false, error: 'no such channel' }
  mockAlertChannelsData = mockAlertChannelsData.filter((c) => c.id !== id)
  return { ok: true }
}

export function mockTestAlertChannel(id: string): MutationResult {
  if (!mockAlertChannelsData.some((c) => c.id === id)) return { ok: false, error: 'no such channel' }
  return { ok: true }
}

export function mockAlertIncidents(filters: AlertIncidentsFilter = {}): AlertIncident[] {
  let rows = [...mockAlertIncidentsData]
  if (filters.status === 'triggered') rows = rows.filter((i) => i.ended_at == null)
  else if (filters.status === 'resolved') rows = rows.filter((i) => i.ended_at != null)
  if (filters.rule_id) rows = rows.filter((i) => i.rule_id === filters.rule_id)
  rows = rows.sort((a, b) => b.started_at - a.started_at)
  if (filters.limit) rows = rows.slice(0, filters.limit)
  return rows
}
