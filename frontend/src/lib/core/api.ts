// API client. Prefers the real photon-server at /api; falls back to the in-browser mock
// so the UI is fully usable during development. Each method degrades independently.

import { ref } from 'vue'
import ky from 'ky'
import {
  SERVICES,
  queryMock,
  mockFields,
  mockFacet,
  mockHistogram,
  mockTrace,
  mockSearchTraces,
  mockSearchSpans,
  mockTracesFields,
  mockTracesFacet,
  mockTracesHistogram,
  mockTracesLatency,
  mockRed,
  mockMetricCatalog,
  mockMetricMetadata,
  mockMetricLabels,
  mockMetricQuery,
  mockStorage,
  mockRetention,
  mockUsageSeries,
  mockServiceTimeseries,
  mockMonitors,
  mockRumApps,
  mockCreateRumApp,
  mockUpdateRumApp,
  mockRotateRumAppKey,
  mockDeleteRumApp,
  mockRumVitals,
  mockRumBreakdown,
  mockRumPages,
  mockRumPageDetail,
  mockRumErrors,
  mockRumErrorFacets,
  mockRumErrorDetail,
  mockInfraHosts,
  mockInfraHost,
  mockInfraHostSeries,
  mockAlertRules,
  mockAlertRule,
  mockCreateAlertRule,
  mockUpdateAlertRule,
  mockDeleteAlertRule,
  mockTestAlertRule,
  mockAlertPreview,
  mockAlertChannels,
  mockAlertChannel,
  mockCreateAlertChannel,
  mockUpdateAlertChannel,
  mockDeleteAlertChannel,
  mockTestAlertChannel,
  mockAlertIncidents,
} from '@/lib/core/mock'

// ---------------------------------------------------------------------------
// Types — request params and resolved response payloads for every `api.*` method.
// These describe the shapes the UI actually consumes (e.g. timestamps hydrated to
// BigInt), not necessarily the raw wire JSON. The `Raw*` aliases capture the wire
// shape before hydration where the two differ.
// ---------------------------------------------------------------------------

// Per-call options threaded into every method (cancellation via AbortSignal).
export interface RequestOpts {
  signal?: AbortSignal
}

// The augmented error the `beforeError` hook produces: an `HTTPError` with the old
// `http()` contract re-exposed as `.status` / `.body`. Network-down errors have no
// `response`, so `.status` stays undefined (which is what triggers the mock fallback).
export interface ApiErrorBody {
  error?: string
  offset?: number
}
export interface ApiError extends Error {
  status?: number
  body?: ApiErrorBody
  response?: Response
  data?: unknown
}

// --- Auth / users ---
export interface SessionInfo {
  authenticated: boolean
  username: string | null
  needs_setup: boolean
}
export interface UserInfo {
  username: string
  created_at: number
}
export interface UsersResult {
  users: UserInfo[]
}
// Result of a write that may report a validation error alongside `ok: false`.
export interface MutationResult {
  ok: boolean
  error?: string
}

// --- Logs: search / fields / facets / histogram ---
export type FieldKind = 'fixed' | 'promoted' | 'attribute'
export interface FieldInfo {
  name: string
  kind: FieldKind
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

// A log row as it arrives on the wire (`timestamp` is a decimal-nanosecond string).
export interface RawLogRow {
  timestamp: string
  [key: string]: unknown
}
// The same row after hydration: `timestamp` is a BigInt (the UI's ns unit).
export type LogRow = Omit<RawLogRow, 'timestamp'> & { timestamp: bigint }

export interface LogSearchRequest {
  start_ts_nanos: string
  end_ts_nanos: string
  services?: string[]
  severities?: string[]
  query: string
  limit?: number
}
export interface RawLogSearchResponse {
  rows: RawLogRow[]
  matched_count: number
  elapsed_ms: number
}
export interface LogSearchResult {
  rows: LogRow[]
  matched_count: number
  elapsed_ms: number
}

// --- Traces / spans ---
export type TraceSort = 'recent' | 'slowest' | 'errors'

// A span as it arrives on the wire (start/end are decimal-nanosecond strings).
export interface RawSpanRow {
  trace_id: string
  span_id: string
  parent_span_id: string | null
  name: string
  kind: number
  kind_text: string
  start_time_nanos: string
  end_time_nanos: string | null
  duration_nanos: number
  status_code: number
  status_text: string
  status_message: string | null
  scope_name: string
  service: string
  events: unknown[] | null
  links: unknown[] | null
  attributes: Record<string, unknown>
  [key: string]: unknown
}
// The same span after hydration: BigInt start/end nanos.
export type SpanRow = Omit<RawSpanRow, 'start_time_nanos' | 'end_time_nanos'> & {
  start_time_nanos: bigint
  end_time_nanos: bigint | null
}

// A trace roll-up as it arrives on the wire (start_ts/duration_ns are ns strings).
export interface RawTraceSummary {
  trace_id: string
  root_service: string | null
  root_name: string | null
  start_ts: string
  duration_ns: string | null
  span_count: number
  error_count: number
  services: string[]
}
// The same summary after hydration: BigInt start_ts / duration_ns.
export type TraceSummary = Omit<RawTraceSummary, 'start_ts' | 'duration_ns'> & {
  start_ts: bigint
  duration_ns: bigint | null
}

// Shared request envelope for both trace-search and span-search (buildRequest is shared).
export interface SpanSearchRequest {
  start: string
  end: string
  query: string
  sort?: TraceSort
  limit?: number
  cursor?: string | null
  columns?: string[]
}
export type TraceSearchRequest = SpanSearchRequest

export interface RawTraceDetail {
  trace_id: string
  spans: RawSpanRow[]
  elapsed_ms: number
  [key: string]: unknown
}
export interface TraceDetail {
  trace_id: string
  spans: SpanRow[]
  elapsed_ms: number
  [key: string]: unknown
}

export interface RawTraceSearchResponse {
  traces: RawTraceSummary[]
  matched_count: number
  elapsed_ms: number
  next_cursor?: string
}
export interface TraceSearchResult {
  traces: TraceSummary[]
  matched_count: number
  elapsed_ms: number
  next_cursor?: string
}
export interface RawSpanSearchResponse {
  rows: RawSpanRow[]
  matched_count: number
  elapsed_ms: number
  next_cursor?: string
}
export interface SpanSearchResult {
  rows: SpanRow[]
  matched_count: number
  elapsed_ms: number
  next_cursor?: string
}

export interface TracesHistogramBucket {
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
export type RedGroup = 'operation' | 'service'
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

// --- Metrics ---
export interface MetricCatalogFilter {
  search?: string
  type?: string
}
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
export interface MetricLabelsResult {
  keys?: string[]
  values?: string[]
  capped?: boolean
}
export interface MetricQuerySpec {
  id: string
  metric: string
  agg?: string
  group_by?: string[]
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
export interface MetricResult {
  id: string
  series: MetricSeries[]
  default_agg: string
}
export interface MetricQueryResponse {
  results: MetricResult[]
  step: string
  capped: boolean
  elapsed_ms: number
}

// --- Uptime monitors ---
export interface Monitor {
  id: string
  name: string
  type: string
  target: string
  interval_secs: number
  timeout_secs: number
  retries: number
  enabled: boolean
  last_state?: string
  last_check_at?: number
  last_latency_ms?: number
  [key: string]: unknown
}
// Editable monitor payload for create/update (form body — kept loose on purpose).
export interface MonitorInput {
  name?: string
  type?: string
  target?: string
  interval_secs?: number
  timeout_secs?: number
  retries?: number
  enabled?: boolean
  [key: string]: unknown
}
export interface HeartbeatsResult {
  heartbeats: unknown[]
  uptime_pct: number
}

// --- Data / retention / usage / storage ---
export interface SignalStorage {
  file_count?: number
  total_rows?: number
  min_ts_nanos?: number
  max_ts_nanos?: number
  bytes?: number
  durable_bytes?: number
  monitor_count?: number
  heartbeat_count?: number
  incident_count?: number
  oldest_heartbeat_ts?: number
  newest_heartbeat_ts?: number
}
export interface StorageStats {
  signals: Record<string, SignalStorage>
  durable: { configured: boolean; pending: number; last_replicated_ms: number }
}
export interface UsageBucket {
  ts: number
  hot_bytes: number
  durable_bytes: number
  total_rows: number
  ingest_rows: number | null
  ingest_bytes: number | null
}
export interface UsageSeries {
  window: string
  bucket_ms: number
  series: Record<string, UsageBucket[]>
}
export interface Retention {
  logs: number
  traces: number
  metrics: number
  uptime: number
}
export interface SetRetentionResult {
  ok: boolean
  error?: string
  retention?: Retention
}
// Purge is intentionally loose: the request/report shapes are admin-only and evolving.
export type PurgeRequest = Record<string, unknown>
export interface PurgeResult {
  ok: boolean
  error?: string
  report?: unknown
}

// --- Services (APM) ---
export interface ServiceTimeseriesRange {
  start?: string
  end?: string
  buckets?: number
}
export interface ServiceTimeseriesBucket {
  ts: number
  rate: number
  error_rate: number
  p50: string
  p90: string
  p99: string
}
export interface ServiceDependencies {
  database: unknown[]
  external: unknown[]
}
export interface ServiceSettings {
  apdex_threshold_ms: number
  is_default: boolean
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
export interface RumAppsResult {
  apps: RumApp[]
}
export interface RumAppInput {
  allowed_origins: string[]
  sample_rate?: number
  rate_limit?: number
}
export interface RumRotateKeyResult {
  key: string
}
export interface RumVitalDist {
  good: number
  needs: number
  poor: number
  total?: number
}
export interface RumVitalRow {
  metric: string
  p75: number
  rating: string
  good_max: number
  poor_min: number
  dist: RumVitalDist
}
export interface RumVitalsResult {
  app: string
  vitals: RumVitalRow[]
}
export interface RumBreakdownRow {
  key: string
  pageviews: number
  lcp_p75: number
  inp_p75: number
  cls_p75: number
}
export interface RumBreakdownResult {
  app: string
  dimension: string
  rows: RumBreakdownRow[]
}
export interface RumPage {
  route: string
  pageviews: number
  lcp_p75: number
  inp_p75: number
  cls_p75: number
}
export interface RumPagesResult {
  app: string
  pages: RumPage[]
}
export interface RumError {
  fingerprint: string
  exception_type: string
  message: string
  count: number
  sessions: number
  trace_id?: string
}
export interface RumErrorsResult {
  app: string
  errors: RumError[]
}
export interface RumErrorFacetsResult {
  app: string
  facets: Record<string, { values: RumErrorTagValue[]; capped: boolean }>
}
export interface RumPageVitals {
  pageviews: number
  lcp_p75: number
  inp_p75: number
  cls_p75: number
}
export interface RumLcpAttribution {
  ttfb: number | null
  resource_load_delay: number | null
  resource_load_time: number | null
  element_render_delay: number | null
  element: string | null
}
export interface RumPageDetailResult {
  app: string
  route: string
  vitals: RumPageVitals | null
  breakdown: RumBreakdownRow[]
  errors: RumError[]
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
export interface RumErrorDetailResult {
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

// --- Infrastructure (host/GPU resource monitoring) ---
export interface InfraHost {
  host: string
  cpuUtil: number | null
  memUtil: number | null
  lastSeenNs: string
  hasGpu: boolean
}
export interface InfraHostsResult {
  hosts: InfraHost[]
}
export interface InfraHostDetail {
  host: string
  os: string | null
  cores: number | null
  totalRamBytes: number | null
  gpus: string[]
  lastSeenNs: string
}
export interface InfraSeriesResult {
  resource: string
  series: MetricSeries[]
}

// --- Alerts (webhook alert & notification engine) ---
// Field names/shapes mirror docs/superpowers/specs/2026-07-18-webhook-alert-notifications-design.md
// §5 (data model) and §5.1 (per-signal `condition` JSON) exactly — the backend (not yet built)
// is expected to match this contract.
export type AlertSignal = 'metrics' | 'logs' | 'traces' | 'rum'
export type AlertCmp = 'gt' | 'gte' | 'lt' | 'lte'
export type AlertSeverity = 'info' | 'warning' | 'critical'

export interface MetricsCondition {
  signal: 'metrics'
  metric_name: string
  label_filters?: Record<string, string>
  group_by?: string[]
  agg: 'avg' | 'min' | 'max' | 'sum' | 'last' | 'p50' | 'p90' | 'p95' | 'p99' | 'rate' | 'increase'
  window_secs: number
  cmp: AlertCmp
  threshold: number
}
export interface LogsCondition {
  signal: 'logs'
  query: string
  group_by?: string | null
  window_secs: number
  cmp: AlertCmp
  threshold: number
}
export interface TracesCondition {
  signal: 'traces'
  service: string
  operation?: string | null
  kind: 'error_rate' | 'latency_p50' | 'latency_p90' | 'latency_p95' | 'latency_p99' | 'request_rate'
  window_secs: number
  cmp: AlertCmp
  threshold: number
}
export interface RumCondition {
  signal: 'rum'
  app_id: string
  route?: string | null
  kind: 'vital_lcp_p75' | 'vital_inp_p75' | 'vital_cls_p75' | 'vital_fcp_p75' | 'vital_ttfb_p75' | 'error_count'
  window_secs: number
  cmp: AlertCmp
  threshold: number
}
// Discriminated on `signal`, matching the Rust `#[serde(tag = "signal")] enum Condition`.
export type AlertCondition = MetricsCondition | LogsCondition | TracesCondition | RumCondition

export interface AlertRule {
  id: string
  name: string
  description: string | null
  enabled: boolean
  signal: AlertSignal
  condition: AlertCondition
  for_secs: number
  interval_secs: number
  severity: AlertSeverity
  channel_ids: string[]
  created_at: number
  updated_at: number
}
// Editable rule payload for create/update (form body — all optional so the same type covers both
// a full create submit and a partial PATCH, e.g. the enable/pause toggle sending only `enabled`).
export interface AlertRuleInput {
  name?: string
  description?: string | null
  enabled?: boolean
  signal?: AlertSignal
  condition?: AlertCondition
  for_secs?: number
  interval_secs?: number
  severity?: AlertSeverity
  channel_ids?: string[]
}

export type ChannelKind = 'webhook' | 'discord' | 'telegram'

export type ChannelConfig =
  | { type: 'webhook'; url: string; secret?: string | null; headers?: Record<string, string> | null }
  | { type: 'discord'; webhook_url: string }
  | { type: 'telegram'; bot_token: string; chat_id: string }

export interface AlertChannel {
  id: string
  name: string
  kind: ChannelKind
  config: ChannelConfig
  created_at: number
  updated_at: number
}
export interface AlertChannelInput {
  name: string
  config: ChannelConfig
}

export interface AlertIncident {
  id: number
  rule_id: string
  series_key: string
  started_at: number
  ended_at: number | null
  peak_value: number
  severity: AlertSeverity
  summary: string
}
export interface AlertIncidentsFilter {
  status?: 'triggered' | 'resolved'
  rule_id?: string
  limit?: number
}

// One series' current value from a condition evaluation — powers the "will trigger on N series
// now" live preview in the create/edit dialog (`POST /api/alerts/preview`) and the saved-rule test
// action (`POST /api/alerts/rules/:id/test`).
export interface AlertPreviewSeries {
  series_key: Record<string, string>
  value: number
  breaching: boolean
}
export interface AlertPreviewResult {
  series: AlertPreviewSeries[]
}

export interface AlertRuleResult extends MutationResult {
  rule?: AlertRule
}
export interface AlertChannelResult extends MutationResult {
  channel?: AlertChannel
}
export interface AlertTestRuleResult extends MutationResult {
  series?: AlertPreviewSeries[]
}

// The per-signal grain of a live-tail stream (matches the search path it merges into).
export type StreamGrain = 'logs' | 'spans'

// ---------------------------------------------------------------------------

// Degraded-mode flag: `true` when a request fell back to the in-browser mock corpus because
// `/api` was unreachable. Reactive (a `ref` behind the `api.mock` getter/setter below) so the
// "demo mode" badge tracks it live. It is NOT a one-way latch: the `afterResponse` hook clears
// it the moment a real 2xx comes back, so a tab self-heals once the backend returns (e.g. after
// a `make dev` startup race or an F5 backend rebuild) — no page reload needed.
const mockState = ref(false)

// Shared Ky instance. The `afterResponse` hook recovers from mock mode on any real 2xx. The
// `beforeError` hook reproduces the old `http()` error contract: on a non-2xx response it
// attaches `.status` and (when present) a parsed JSON `.body`, so callers can keep doing
// `e.status === 400` and reading `e.body?.error` / `e.body?.offset`. Network-down errors have
// no `response`, so `.status` stays undefined and each method's mock fallback still triggers.
// TanStack Query owns retries, so `retry: 0` here.
const http = ky.create({
  // ky v2 renamed `prefixUrl` → `prefix` and now THROWS in its constructor if `prefixUrl` is
  // passed — which fires on every request before `fetch`, so each call would reject and fall back
  // to the mock corpus (whole app stuck in "demo mode", writes silently dropped). Same join
  // semantics: `prefix:'/api'` + `'monitors/:id/pause'` → `/api/monitors/:id/pause`.
  prefix: '/api',
  credentials: 'same-origin',
  retry: 0,
  // ky v2 hooks take a single state object (`{ request, options, response, error, ... }`), NOT the
  // v1 positional `(request, options, response)` / `(error)` args.
  hooks: {
    afterResponse: [
      ({ response }) => {
        // A real 2xx means the backend is reachable AND we're authenticated — definitely not
        // demo mode, so drop any prior mock latch.
        if (response.ok) mockState.value = false
      },
    ],
    beforeError: [
      ({ error }) => {
        // On an HTTP error ky throws an `HTTPError` carrying `.response`; network-down errors have
        // none, so `.status` stays undefined and each method's mock fallback still triggers.
        // ky v2 consumes the body to pre-parse it into `error.data` (so `response.json()` no longer
        // works) — re-expose it as the `.status` / `.body` the callers already read
        // (`e.status === 400`, `e.body?.error` / `e.body?.offset`). `.data` is undefined for empty
        // or non-JSON bodies, which is exactly the old contract.
        const e = error as ApiError
        const response = e.response
        if (response) {
          e.status = response.status
          e.body = e.data as ApiErrorBody
        }
        return error
      },
    ],
  },
})

// Convert API rows (timestamp as string nanos) into the shape the UI uses (BigInt). Accepts the
// mock corpus's already-BigInt timestamps too (BigInt() is a no-op on a bigint), so this bridges
// both the wire path and the mock fallback — hence the loose `any[]` input (the two corpora carry
// slightly different row types).
function hydrate(rows: any[]): LogRow[] {
  return rows.map((r) => ({ ...r, timestamp: BigInt(r.timestamp) })) as LogRow[]
}

// Convert API spans (string nanos) into UI shape (BigInt start/end). Loose `any[]` input for the
// same reason as `hydrate` (wire `RawSpanRow` vs the mock corpus's span shape).
function hydrateSpans(spans: any[]): SpanRow[] {
  return spans.map((s) => ({
    ...s,
    start_time_nanos: BigInt(s.start_time_nanos),
    end_time_nanos: s.end_time_nanos == null ? null : BigInt(s.end_time_nanos),
  })) as SpanRow[]
}

// Hydrate live-streamed rows into the SAME UI shape (BigInt nanos) the search path produces, so a
// merged table of streamed + searched rows never mixes BigInt and string timestamps (which throws
// in the ns→ms BigInt math in `format.js`). Keyed by grain to match the corresponding search call.
export function hydrateStreamRows(grain: string, rows: any[]): Array<Record<string, unknown>> {
  return grain === 'spans' ? hydrateSpans(rows) : hydrate(rows)
}

// Convert API trace summaries (string start_ts/duration_ns) into UI shape (BigInt). Loose `any[]`
// input to bridge the wire `RawTraceSummary` and the mock corpus's summary shape.
function hydrateTraces(traces: any[]): TraceSummary[] {
  return traces.map((t) => ({
    ...t,
    start_ts: BigInt(t.start_ts),
    duration_ns: t.duration_ns == null ? null : BigInt(t.duration_ns),
  })) as TraceSummary[]
}

let mockUsers: UserInfo[] = [{ username: 'demo', created_at: Date.now() }]

// Local mutable copy of `mockRetention` (imported `let` bindings from another ES module are
// live-read but not reassignable from here, so — like `mockUsers` above — the mutable state
// lives in this module and is only seeded from the mock.js default).
let mockRetentionState: Retention = { ...mockRetention }

export const api = {
  // Reactive degraded-mode indicator. Getter/setter over `mockState` so the ~22 `api.mock = true`
  // fallback sites and the `:mock="api.mock"` template bindings stay unchanged while the value is
  // fully reactive (badge updates both when we drop into mock mode and when we recover from it).
  get mock(): boolean {
    return mockState.value
  },
  set mock(v: boolean) {
    mockState.value = v
  },

  async login(username: string, password: string, opts: RequestOpts = {}): Promise<{ ok: boolean }> {
    try {
      await http.post('login', { json: { username, password }, signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      // In mock mode any credentials sign in — there is no server to check against.
      if (e.status === 401) return { ok: false }
      api.mock = true
      return { ok: true }
    }
  },

  async logout(opts: RequestOpts = {}): Promise<void> {
    try {
      await http.post('logout', { signal: opts.signal })
    } catch {
      /* mock mode — nothing to tear down */
    }
  },

  // Boot probe: is the user logged in, and does the instance need first-run onboarding? The
  // session cookie is httpOnly (JS can't read it), so the SPA asks the server on every load.
  async session(opts: RequestOpts = {}): Promise<SessionInfo> {
    try {
      return await http.get('session', { signal: opts.signal }).json<SessionInfo>()
    } catch {
      // No backend (pure-frontend dev): behave as an already-onboarded, authenticated demo.
      api.mock = true
      return { authenticated: true, username: 'demo', needs_setup: false }
    }
  },

  // First-run onboarding: create the very first account. Returns { ok, error? }.
  async setup(username: string, password: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.post('setup', { json: { username, password }, signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 400 || e.status === 409) return { ok: false, error: e.body?.error }
      api.mock = true
      return { ok: true }
    }
  },

  async listUsers(opts: RequestOpts = {}): Promise<UsersResult> {
    try {
      return await http.get('users', { signal: opts.signal }).json<UsersResult>()
    } catch {
      api.mock = true
      return { users: mockUsers }
    }
  },

  // Returns { ok, error? } — 400/409 are surfaced (not mocked); a network failure mocks success.
  async createUser(username: string, password: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.post('users', { json: { username, password }, signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 400 || e.status === 409) return { ok: false, error: e.body?.error }
      api.mock = true
      mockUsers = [...mockUsers, { username, created_at: Date.now() }]
      return { ok: true }
    }
  },

  async deleteUser(username: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`users/${encodeURIComponent(username)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      mockUsers = mockUsers.filter((u) => u.username !== username)
      return { ok: true }
    }
  },

  async services(opts: RequestOpts = {}): Promise<string[]> {
    try {
      return await http.get('services', { signal: opts.signal }).json<string[]>()
    } catch {
      api.mock = true
      return SERVICES
    }
  },

  async search(request: LogSearchRequest, opts: RequestOpts = {}): Promise<LogSearchResult> {
    try {
      const res = await http.post('search', { json: request, signal: opts.signal }).json<RawLogSearchResponse>()
      return { rows: hydrate(res.rows), matched_count: res.matched_count, elapsed_ms: res.elapsed_ms }
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      await new Promise((r) => setTimeout(r, 180))
      const m = queryMock(request)
      return { rows: hydrate(m.rows), matched_count: m.matched_count, elapsed_ms: m.elapsed_ms }
    }
  },

  async fields(startNs: string, endNs: string, opts: RequestOpts = {}): Promise<FieldInfo[]> {
    try {
      return await http.get('fields', { searchParams: { start: startNs, end: endNs }, signal: opts.signal }).json<FieldInfo[]>()
    } catch {
      api.mock = true
      return mockFields() as FieldInfo[]
    }
  },

  async facet(field: string, query: string, startNs: string, endNs: string, limit = 50, opts: RequestOpts = {}): Promise<FacetResult> {
    try {
      return await http
        .get('facet', { searchParams: { field, query, start: startNs, end: endNs, limit }, signal: opts.signal })
        .json<FacetResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockFacet(field, query, startNs, endNs, limit) as FacetResult
    }
  },

  async histogram(query: string, startNs: string, endNs: string, buckets = 48, opts: RequestOpts = {}): Promise<LogHistogramBucket[]> {
    try {
      return await http
        .get('histogram', { searchParams: { query, start: startNs, end: endNs, buckets }, signal: opts.signal })
        .json<LogHistogramBucket[]>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockHistogram(query, startNs, endNs, buckets) as LogHistogramBucket[]
    }
  },

  async getTrace(traceId: string, timeHintNs?: string | null, opts: RequestOpts = {}): Promise<TraceDetail> {
    try {
      const res = await http
        .get(`traces/${encodeURIComponent(traceId)}`, {
          searchParams: timeHintNs ? { time_hint: timeHintNs } : {},
          signal: opts.signal,
        })
        .json<RawTraceDetail>()
      return { ...res, spans: hydrateSpans(res.spans) }
    } catch (e: any) {
      if (e.status === 404) throw e // real "trace not found" — surface it, don't mock
      api.mock = true
      await new Promise((r) => setTimeout(r, 120))
      const m = mockTrace(traceId)
      return { ...m, spans: hydrateSpans(m.spans) }
    }
  },

  async searchTraces(request: TraceSearchRequest, opts: RequestOpts = {}): Promise<TraceSearchResult> {
    try {
      const res = await http.post('traces/search', { json: request, signal: opts.signal }).json<RawTraceSearchResponse>()
      return {
        traces: hydrateTraces(res.traces),
        matched_count: res.matched_count,
        elapsed_ms: res.elapsed_ms,
        next_cursor: res.next_cursor,
      }
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      await new Promise((r) => setTimeout(r, 180))
      const m = mockSearchTraces(request as any)
      return {
        traces: hydrateTraces(m.traces),
        matched_count: m.matched_count,
        elapsed_ms: m.elapsed_ms,
        next_cursor: m.next_cursor,
      }
    }
  },

  async searchSpans(request: SpanSearchRequest, opts: RequestOpts = {}): Promise<SpanSearchResult> {
    try {
      const res = await http.post('spans/search', { json: request, signal: opts.signal }).json<RawSpanSearchResponse>()
      return {
        rows: hydrateSpans(res.rows),
        matched_count: res.matched_count,
        elapsed_ms: res.elapsed_ms,
        next_cursor: res.next_cursor,
      }
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      await new Promise((r) => setTimeout(r, 180))
      const m = mockSearchSpans(request as any)
      return {
        rows: hydrateSpans(m.rows),
        matched_count: m.matched_count,
        elapsed_ms: m.elapsed_ms,
        next_cursor: m.next_cursor,
      }
    }
  },

  async tracesFields(startNs: string, endNs: string, opts: RequestOpts = {}): Promise<FieldInfo[]> {
    try {
      return await http
        .get('traces/fields', { searchParams: { start: startNs, end: endNs }, signal: opts.signal })
        .json<FieldInfo[]>()
    } catch {
      api.mock = true
      return mockTracesFields() as FieldInfo[]
    }
  },

  async tracesFacet(field: string, query: string, startNs: string, endNs: string, limit = 50, opts: RequestOpts = {}): Promise<FacetResult> {
    try {
      return await http
        .get('traces/facet', { searchParams: { field, query, start: startNs, end: endNs, limit }, signal: opts.signal })
        .json<FacetResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockTracesFacet(field, query, startNs, endNs, limit) as FacetResult
    }
  },

  async tracesHistogram(query: string, startNs: string, endNs: string, buckets = 48, opts: RequestOpts = {}): Promise<TracesHistogramBucket[]> {
    try {
      return await http
        .get('traces/histogram', { searchParams: { query, start: startNs, end: endNs, buckets }, signal: opts.signal })
        .json<TracesHistogramBucket[]>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockTracesHistogram(query, startNs, endNs, buckets) as TracesHistogramBucket[]
    }
  },

  async tracesLatency(query: string, startNs: string, endNs: string, buckets = 48, opts: RequestOpts = {}): Promise<LatencyResult> {
    try {
      return await http
        .get('traces/latency', { searchParams: { query, start: startNs, end: endNs, buckets }, signal: opts.signal })
        .json<LatencyResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockTracesLatency(query, startNs, endNs, buckets) as LatencyResult
    }
  },

  async red(query: string, startNs: string, endNs: string, group: RedGroup = 'operation', opts: RequestOpts = {}): Promise<RedRow[]> {
    try {
      return await http
        .get('red', { searchParams: { query, start: startNs, end: endNs, group }, signal: opts.signal })
        .json<RedRow[]>()
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      return mockRed(query, startNs, endNs, group) as RedRow[]
    }
  },

  async metricCatalog(startNs: string, endNs: string, { search, type }: MetricCatalogFilter = {}, opts: RequestOpts = {}): Promise<MetricCatalogEntry[]> {
    const searchParams: Record<string, string | number> = { start: startNs, end: endNs }
    if (search) searchParams.search = search
    if (type) searchParams.type = type
    try {
      return await http.get('metrics/catalog', { searchParams, signal: opts.signal }).json<MetricCatalogEntry[]>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockMetricCatalog(startNs, endNs, { search, type }) as MetricCatalogEntry[]
    }
  },

  async metricMetadata(name: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<MetricMetadata> {
    try {
      return await http
        .get(`metrics/metadata/${encodeURIComponent(name)}`, {
          searchParams: { start: startNs, end: endNs }, signal: opts.signal,
        })
        .json<MetricMetadata>()
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) throw e
      api.mock = true
      const md = mockMetricMetadata(name, startNs, endNs)
      if (!md) { const err = new Error('unknown metric') as ApiError; err.status = 404; throw err }
      return md as MetricMetadata
    }
  },

  async metricLabels(metric: string, key: string | null, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<MetricLabelsResult> {
    const searchParams: Record<string, string | number> = { metric, start: startNs, end: endNs }
    if (key) searchParams.key = key
    try {
      return await http.get('metrics/labels', { searchParams, signal: opts.signal }).json<MetricLabelsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockMetricLabels(metric, key, startNs, endNs) as MetricLabelsResult
    }
  },

  async metricQuery(request: MetricQueryRequest, opts: RequestOpts = {}): Promise<MetricQueryResponse> {
    try {
      return await http.post('metrics/query', { json: request, signal: opts.signal }).json<MetricQueryResponse>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockMetricQuery(request) as MetricQueryResponse
    }
  },

  async listMonitors(opts: RequestOpts = {}): Promise<Monitor[]> {
    try {
      return await http.get('monitors', { signal: opts.signal }).json<Monitor[]>()
    } catch {
      api.mock = true
      return mockMonitors as Monitor[]
    }
  },

  async getMonitor(id: string, opts: RequestOpts = {}): Promise<Monitor> {
    try {
      return await http.get(`monitors/${id}`, { signal: opts.signal }).json<Monitor>()
    } catch (e: any) {
      if (e.status === 404) throw e // real "monitor not found" — surface it, don't mock
      api.mock = true
      return (mockMonitors.find((m) => m.id === id) ?? mockMonitors[0]) as Monitor
    }
  },

  async createMonitor(body: MonitorInput, opts: RequestOpts = {}): Promise<Monitor> {
    return http.post('monitors', { json: body, signal: opts.signal }).json<Monitor>()
  },

  async updateMonitor(id: string, body: MonitorInput, opts: RequestOpts = {}): Promise<Monitor> {
    return http.patch(`monitors/${id}`, { json: body, signal: opts.signal }).json<Monitor>()
  },

  async deleteMonitor(id: string, opts: RequestOpts = {}): Promise<boolean> {
    await http.delete(`monitors/${id}`, { signal: opts.signal })
    return true
  },

  async pauseMonitor(id: string, opts: RequestOpts = {}): Promise<Monitor> {
    return http.post(`monitors/${id}/pause`, { signal: opts.signal }).json<Monitor>()
  },

  async resumeMonitor(id: string, opts: RequestOpts = {}): Promise<Monitor> {
    return http.post(`monitors/${id}/resume`, { signal: opts.signal }).json<Monitor>()
  },

  async getHeartbeats(id: string, window = '24h', opts: RequestOpts = {}): Promise<HeartbeatsResult> {
    try {
      return await http
        .get(`monitors/${id}/heartbeats`, { searchParams: { window }, signal: opts.signal })
        .json<HeartbeatsResult>()
    } catch {
      api.mock = true
      return { heartbeats: [], uptime_pct: 100 }
    }
  },

  async getIncidents(id: string, opts: RequestOpts = {}): Promise<unknown[]> {
    try {
      return await http.get(`monitors/${id}/incidents`, { signal: opts.signal }).json<unknown[]>()
    } catch {
      api.mock = true
      return []
    }
  },

  async getStorage(opts: RequestOpts = {}): Promise<StorageStats> {
    try {
      return await http.get('storage', { signal: opts.signal }).json<StorageStats>()
    } catch {
      api.mock = true
      return mockStorage as StorageStats
    }
  },

  async getUsageSeries({ window = '24h' }: { window?: string } = {}, opts: RequestOpts = {}): Promise<UsageSeries> {
    try {
      return await http.get('usage/series', { searchParams: { window }, signal: opts.signal }).json<UsageSeries>()
    } catch {
      api.mock = true
      return mockUsageSeries(window) as UsageSeries
    }
  },

  async getRetention(opts: RequestOpts = {}): Promise<Retention> {
    try {
      return await http.get('retention', { signal: opts.signal }).json<Retention>()
    } catch {
      api.mock = true
      return { ...mockRetentionState }
    }
  },

  // Returns { ok, error? }. 400 is surfaced; a network failure mocks success.
  async setRetention(partial: Partial<Retention>, opts: RequestOpts = {}): Promise<SetRetentionResult> {
    try {
      const next = await http.put('retention', { json: partial, signal: opts.signal }).json<Retention>()
      return { ok: true, retention: next }
    } catch (e: any) {
      if (e.status === 400) return { ok: false, error: e.body?.error }
      api.mock = true
      mockRetentionState = { ...mockRetentionState, ...partial }
      return { ok: true, retention: { ...mockRetentionState } }
    }
  },

  // Returns { ok, error?, report? }. 400 is surfaced; a network failure mocks success.
  async purgeData(body: PurgeRequest, opts: RequestOpts = {}): Promise<PurgeResult> {
    try {
      const report = await http.post('data/purge', { json: body, signal: opts.signal }).json()
      return { ok: true, report }
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return { ok: true, report: {} }
    }
  },

  async serviceTimeseries(service: string, { start, end, buckets }: ServiceTimeseriesRange = {}, opts: RequestOpts = {}): Promise<ServiceTimeseriesBucket[]> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/timeseries`, {
          searchParams: { start, end, buckets }, signal: opts.signal,
        })
        .json<ServiceTimeseriesBucket[]>()
    } catch {
      api.mock = true
      return mockServiceTimeseries(service, buckets, { start, end }) as ServiceTimeseriesBucket[]
    }
  },

  async serviceDependencies(service: string, { start, end }: { start?: string; end?: string } = {}, opts: RequestOpts = {}): Promise<ServiceDependencies> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/dependencies`, {
          searchParams: { start, end }, signal: opts.signal,
        })
        .json<ServiceDependencies>()
    } catch {
      api.mock = true
      return { database: [], external: [] }
    }
  },

  async serviceSettings(service: string, opts: RequestOpts = {}): Promise<ServiceSettings> {
    try {
      return await http.get(`services/${encodeURIComponent(service)}/settings`, { signal: opts.signal }).json<ServiceSettings>()
    } catch {
      api.mock = true
      return { apdex_threshold_ms: 500, is_default: true }
    }
  },

  async setServiceSettings(service: string, ms: number, opts: RequestOpts = {}): Promise<ServiceSettings> {
    try {
      return await http
        .put(`services/${encodeURIComponent(service)}/settings`, {
          json: { apdex_threshold_ms: ms }, signal: opts.signal,
        })
        .json<ServiceSettings>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return { apdex_threshold_ms: ms, is_default: false }
    }
  },

  async resetServiceSettings(service: string, opts: RequestOpts = {}): Promise<ServiceSettings> {
    try {
      return await http.delete(`services/${encodeURIComponent(service)}/settings`, { signal: opts.signal }).json<ServiceSettings>()
    } catch {
      api.mock = true
      return { apdex_threshold_ms: 500, is_default: true }
    }
  },

  // --- RUM (Real User Monitoring) ---

  async rumApps(opts: RequestOpts = {}): Promise<RumAppsResult> {
    try {
      return await http.get('rum/apps', { signal: opts.signal }).json<RumAppsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumApps() as RumAppsResult
    }
  },

  async rumCreateApp(name: string, input: RumAppInput, opts: RequestOpts = {}): Promise<MutationResult & { key?: string }> {
    try {
      const res = await http.post('rum/apps', { json: { name, ...input }, signal: opts.signal }).json<RumApp>()
      return { ok: true, key: res.key }
    } catch (e: any) {
      if (e.status === 400 || e.status === 409) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockCreateRumApp(name, input)
    }
  },

  async rumUpdateApp(name: string, input: RumAppInput, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.patch(`rum/apps/${encodeURIComponent(name)}`, { json: input, signal: opts.signal }).json<RumApp>()
      return { ok: true }
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockUpdateRumApp(name, input)
    }
  },

  async rumRotateAppKey(name: string, opts: RequestOpts = {}): Promise<MutationResult & { key?: string }> {
    try {
      const res = await http.post(`rum/apps/${encodeURIComponent(name)}/rotate-key`, { signal: opts.signal }).json<RumRotateKeyResult>()
      return { ok: true, key: res.key }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockRotateRumAppKey(name)
    }
  },

  async rumDeleteApp(name: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`rum/apps/${encodeURIComponent(name)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockDeleteRumApp(name)
    }
  },

  async rumVitals(app: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<RumVitalsResult> {
    try {
      return await http
        .get('rum/vitals', { searchParams: { app, start: startNs, end: endNs }, signal: opts.signal })
        .json<RumVitalsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumVitals(app) as RumVitalsResult
    }
  },

  async rumBreakdown(app: string, dimension: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<RumBreakdownResult> {
    try {
      return await http
        .get('rum/vitals/breakdown', {
          searchParams: { app, dimension, start: startNs, end: endNs },
          signal: opts.signal,
        })
        .json<RumBreakdownResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumBreakdown(app, dimension) as RumBreakdownResult
    }
  },

  async rumPages(app: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<RumPagesResult> {
    try {
      return await http
        .get('rum/pages', { searchParams: { app, start: startNs, end: endNs }, signal: opts.signal })
        .json<RumPagesResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumPages(app) as RumPagesResult
    }
  },

  async rumPageDetail(app: string, route: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<RumPageDetailResult> {
    try {
      return await http
        .get('rum/pages/detail', {
          searchParams: { app, route, start: startNs, end: endNs },
          signal: opts.signal,
        })
        .json<RumPageDetailResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumPageDetail(app, route) as RumPageDetailResult
    }
  },

  async rumErrors(app: string, startNs: string, endNs: string, opts: RequestOpts = {}, q?: string): Promise<RumErrorsResult> {
    try {
      const searchParams: Record<string, string> = { app, start: startNs, end: endNs }
      if (q && q.trim()) searchParams.q = q
      return await http.get('rum/errors', { searchParams, signal: opts.signal }).json<RumErrorsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumErrors(app, q) as RumErrorsResult
    }
  },

  async rumErrorFacets(app: string, q: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<RumErrorFacetsResult> {
    try {
      const searchParams: Record<string, string> = { app, start: startNs, end: endNs }
      if (q && q.trim()) searchParams.q = q
      return await http.get('rum/errors/facets', { searchParams, signal: opts.signal }).json<RumErrorFacetsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumErrorFacets(app) as RumErrorFacetsResult
    }
  },

  async rumErrorDetail(
    app: string,
    fingerprint: string,
    startNs: string,
    endNs: string,
    opts: RequestOpts = {},
  ): Promise<RumErrorDetailResult> {
    try {
      return await http
        .get(`rum/errors/${encodeURIComponent(fingerprint)}`, {
          searchParams: { app, start: startNs, end: endNs },
          signal: opts.signal,
        })
        .json<RumErrorDetailResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumErrorDetail(app, fingerprint) as RumErrorDetailResult
    }
  },

  // --- Infrastructure (host/GPU resource monitoring) ---

  async infraHosts(startNs: string, endNs: string, opts: RequestOpts = {}): Promise<InfraHostsResult> {
    try {
      return await http
        .get('infra/hosts', { searchParams: { start: startNs, end: endNs }, signal: opts.signal })
        .json<InfraHostsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockInfraHosts() as InfraHostsResult
    }
  },

  async infraHost(host: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<InfraHostDetail> {
    try {
      return await http
        .get(`infra/hosts/${encodeURIComponent(host)}`, { searchParams: { start: startNs, end: endNs }, signal: opts.signal })
        .json<InfraHostDetail>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockInfraHost(host) as InfraHostDetail
    }
  },

  async infraHostSeries(
    host: string,
    resource: string,
    startNs: string,
    endNs: string,
    opts: RequestOpts = {},
  ): Promise<InfraSeriesResult> {
    try {
      return await http
        .get(`infra/hosts/${encodeURIComponent(host)}/timeseries`, {
          searchParams: { resource, start: startNs, end: endNs },
          signal: opts.signal,
        })
        .json<InfraSeriesResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockInfraHostSeries(host, resource, startNs, endNs) as InfraSeriesResult
    }
  },

  // --- Alerts (webhook alert & notification engine) ---
  // Every mutating method here returns the non-throwing `{ ok, error }` contract (mirrors
  // `rum*App`/`setRetention`/`purgeData`) rather than the throw-and-let-`onError`-catch pattern
  // `createMonitor`/`updateMonitor` use — the alertsQueries.ts mutations depend on this to never
  // reject. Reads (`alertRules`/`getAlertRule`/`alertChannels`/`getAlertChannel`/`alertIncidents`)
  // follow the usual read-then-mock-fallback shape used by `listMonitors`/`getMonitor`.

  async alertRules(opts: RequestOpts = {}): Promise<AlertRule[]> {
    try {
      return await http.get('alerts/rules', { signal: opts.signal }).json<AlertRule[]>()
    } catch {
      api.mock = true
      return mockAlertRules() as AlertRule[]
    }
  },

  async getAlertRule(id: string, opts: RequestOpts = {}): Promise<AlertRule> {
    try {
      return await http.get(`alerts/rules/${encodeURIComponent(id)}`, { signal: opts.signal }).json<AlertRule>()
    } catch (e: any) {
      if (e.status === 404) throw e // real "rule not found" — surface it, don't mock
      api.mock = true
      const r = mockAlertRule(id)
      if (!r) { const err = new Error('rule not found') as ApiError; err.status = 404; throw err }
      return r as AlertRule
    }
  },

  async createAlertRule(input: AlertRuleInput, opts: RequestOpts = {}): Promise<AlertRuleResult> {
    try {
      const rule = await http.post('alerts/rules', { json: input, signal: opts.signal }).json<AlertRule>()
      return { ok: true, rule }
    } catch (e: any) {
      if (e.status === 400 || e.status === 409) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockCreateAlertRule(input) as AlertRuleResult
    }
  },

  async updateAlertRule(id: string, input: AlertRuleInput, opts: RequestOpts = {}): Promise<AlertRuleResult> {
    try {
      const rule = await http
        .patch(`alerts/rules/${encodeURIComponent(id)}`, { json: input, signal: opts.signal })
        .json<AlertRule>()
      return { ok: true, rule }
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockUpdateAlertRule(id, input) as AlertRuleResult
    }
  },

  async deleteAlertRule(id: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`alerts/rules/${encodeURIComponent(id)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockDeleteAlertRule(id) as MutationResult
    }
  },

  // No dedicated pause/resume route like monitors — toggling a rule is a PATCH of just `enabled`.
  async toggleAlertRule(id: string, enabled: boolean, opts: RequestOpts = {}): Promise<AlertRuleResult> {
    try {
      const rule = await http
        .patch(`alerts/rules/${encodeURIComponent(id)}`, { json: { enabled }, signal: opts.signal })
        .json<AlertRule>()
      return { ok: true, rule }
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockUpdateAlertRule(id, { enabled }) as AlertRuleResult
    }
  },

  async testAlertRule(id: string, opts: RequestOpts = {}): Promise<AlertTestRuleResult> {
    try {
      const res = await http
        .post(`alerts/rules/${encodeURIComponent(id)}/test`, { signal: opts.signal })
        .json<AlertPreviewResult>()
      return { ok: true, series: res.series }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockTestAlertRule(id) as AlertTestRuleResult
    }
  },

  // Dry-run a draft condition → current series+values. Used as a live QUERY (usePreview), not a
  // mutation: an invalid draft condition (400, e.g. mid-edit) is surfaced so the dialog can render
  // it, not swallowed into a mock success.
  async alertPreview(condition: AlertCondition, opts: RequestOpts = {}): Promise<AlertPreviewResult> {
    try {
      return await http.post('alerts/preview', { json: condition, signal: opts.signal }).json<AlertPreviewResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockAlertPreview(condition) as AlertPreviewResult
    }
  },

  async alertChannels(opts: RequestOpts = {}): Promise<AlertChannel[]> {
    try {
      return await http.get('alerts/channels', { signal: opts.signal }).json<AlertChannel[]>()
    } catch {
      api.mock = true
      return mockAlertChannels() as AlertChannel[]
    }
  },

  async getAlertChannel(id: string, opts: RequestOpts = {}): Promise<AlertChannel> {
    try {
      return await http.get(`alerts/channels/${encodeURIComponent(id)}`, { signal: opts.signal }).json<AlertChannel>()
    } catch (e: any) {
      if (e.status === 404) throw e
      api.mock = true
      const c = mockAlertChannel(id)
      if (!c) { const err = new Error('channel not found') as ApiError; err.status = 404; throw err }
      return c as AlertChannel
    }
  },

  async createAlertChannel(input: AlertChannelInput, opts: RequestOpts = {}): Promise<AlertChannelResult> {
    try {
      const channel = await http.post('alerts/channels', { json: input, signal: opts.signal }).json<AlertChannel>()
      return { ok: true, channel }
    } catch (e: any) {
      if (e.status === 400 || e.status === 409) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockCreateAlertChannel(input) as AlertChannelResult
    }
  },

  async updateAlertChannel(id: string, input: AlertChannelInput, opts: RequestOpts = {}): Promise<AlertChannelResult> {
    try {
      const channel = await http
        .patch(`alerts/channels/${encodeURIComponent(id)}`, { json: input, signal: opts.signal })
        .json<AlertChannel>()
      return { ok: true, channel }
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockUpdateAlertChannel(id, input) as AlertChannelResult
    }
  },

  async deleteAlertChannel(id: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`alerts/channels/${encodeURIComponent(id)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockDeleteAlertChannel(id) as MutationResult
    }
  },

  async testAlertChannel(id: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      const r = await http
        .post(`alerts/channels/${encodeURIComponent(id)}/test`, { signal: opts.signal })
        .json<{ delivered: boolean; error?: string }>()
      return r.delivered ? { ok: true } : { ok: false, error: r.error ?? 'delivery failed' }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockTestAlertChannel(id) as MutationResult
    }
  },

  async testAlertChannelDraft(input: AlertChannelInput, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      const r = await http
        .post('alerts/channels/test', { json: input, signal: opts.signal })
        .json<{ delivered: boolean; error?: string }>()
      return r.delivered ? { ok: true } : { ok: false, error: r.error ?? 'delivery failed' }
    } catch (e: any) {
      if (e.status === 400) return { ok: false, error: e.body?.error }
      // Offline/mock mode can't actually POST anywhere — treat a draft test as a no-op success.
      api.mock = true
      return { ok: true }
    }
  },

  async alertIncidents(filters: AlertIncidentsFilter = {}, opts: RequestOpts = {}): Promise<AlertIncident[]> {
    const searchParams: Record<string, string | number> = {}
    if (filters.status) searchParams.status = filters.status
    if (filters.rule_id) searchParams.rule_id = filters.rule_id
    if (filters.limit) searchParams.limit = filters.limit
    try {
      return await http.get('alerts/incidents', { searchParams, signal: opts.signal }).json<AlertIncident[]>()
    } catch {
      api.mock = true
      return mockAlertIncidents(filters) as AlertIncident[]
    }
  },
}
