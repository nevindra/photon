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
  mockLatencyHeatmap,
  mockBackendIssues,
  mockBackendIssueFacets,
  mockBackendIssueDetail,
  mockServiceMap,
  mockDeployments,
  mockSlos,
  mockSloBudget,
  mockServiceUsage,
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
  mockTenants,
  mockCreateTenant,
  mockUpdateTenant,
  mockRotateTenantToken,
  mockDeleteTenant,
  mockTenantsSummary,
  mockFederationStatus,
  mockInfraHosts,
  mockInfraHost,
  mockInfraHostSeries,
  mockInfraHostProcesses,
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
// The Docker fleet-management mock corpus is loaded lazily: it is ~2,300 lines that a real
// install with the backend never needs, so it stays out of the main chunk (see `deployRead`).
type DeployMocks = typeof import('@/lib/core/mockDeploy')
const deployMocks = (): Promise<DeployMocks> => import('@/lib/core/mockDeploy')
import type { Role } from '@/lib/deploy/roles'
export type { Role }
import { DeployUnavailableError, DEPLOY_UNAVAILABLE_MESSAGE, deployDemoFlag } from '@/lib/deploy/availability'

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
  // Absent on a backend that predates roles: the single implicit role of that era could do
  // everything the UI offered, so `auth.ts` reads a missing role as `admin`, not `viewer`.
  role?: Role
}
export interface UserInfo {
  username: string
  created_at: number
  role?: Role
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
/// Which read path produced a row: the `photon.service.*` rollup, or a raw span scan.
/// See docs/subsystems/rollup.md.
export type RedSource = 'rollup' | 'raw'
export interface RedRow {
  service: string
  tenant?: string | null
  /** `deployment.environment`; present for `group=service` only. */
  environment?: string | null
  operation: string | null
  count: number
  rate: number
  /** Every span the service emitted, all kinds — beside `count`, never instead of it (ADR-0003). */
  span_count?: number
  /** Which cascade rule produced `count`. `all_spans` earns a `span-based` badge. */
  basis?: 'entry' | 'trace_root' | 'all_spans'
  error_count: number
  error_rate: number
  http_4xx?: number
  http_5xx?: number
  satisfied?: number
  tolerating?: number
  frustrated?: number
  apdex?: number | null
  p50: string
  p90: string
  p99: string
  source?: RedSource
}

// --- Services index (`GET /api/services/overview`) ---
/** `[epoch ms, value]`, the shape the chart components already read. */
export type SparkPoint = [number, number]
/**
 * One Site as the index's band needs it: registry facts only.
 *
 * No per-Site totals here on purpose — they live in `rows`, which the fleet merge walks anyway.
 * Two separately-computed copies of "how many requests did jkt serve" is how a band and the table
 * under it start disagreeing inside one response.
 */
export interface OverviewSite {
  name: string
  status: 'up' | 'stale' | 'down'
  last_seen_ms: number
  mode: string | null
  ui_url: string | null
}
export interface OverviewRow extends RedRow {
  spark: SparkPoint[]
  error_spark: SparkPoint[]
}
export interface ServicesOverview {
  source: RedSource
  window: { start: string; end: string; buckets: number }
  sites: OverviewSite[]
  rows: OverviewRow[]
}
export interface ServiceOperations {
  source: RedSource
  window: { start: string; end: string; buckets: number }
  rows: OverviewRow[]
}
/**
 * A latency heatmap: `[timeBucketIndex, bandIndex, count]` cells over a fixed latency ladder.
 *
 * Cells are SPARSE — an empty cell is absent, not zero. `bounds` are the ladder's inclusive upper
 * edges as nanosecond STRINGS (JS-safe); band `bounds.length` is everything above the last edge.
 * `open_spans` are spans that never closed, counted apart from every band because "under 1ms" and
 * "never finished" are opposite facts.
 */
export type HeatCell = [number, number, number]
export interface LatencyHeatmap {
  /** Bucket START timestamps, epoch ms. */
  columns: number[]
  bounds: string[]
  cells: HeatCell[]
  open_spans: number
}

// --- Backend Issues (`GET /api/services[/:service]/errors*`) ---
/**
 * One Backend Issue: every errored span sharing `(service, fingerprint)` in the window.
 *
 * Identity is the PAIR. The fingerprint hashes the exception and nothing about where it ran, so
 * the same exception in two services is two Issues — two owners, two deploys, two fixes. Rows on
 * the fleet list therefore always name their own service.
 *
 * `traces` is the backend's answer to RUM's `sessions`: how many requests went wrong, where
 * `count` also counts a retry loop inside one of them.
 */
export interface BackendIssue {
  service: string
  fingerprint: string
  error_type: string
  message: string
  count: number
  traces: number
  /** Distinct operations this Issue reaches. `> 1` means one fault, several call sites. */
  operations: number
  /** Epoch NANOSECOND strings (JS-safe), like every other span timestamp on this surface. */
  first_seen: string
  last_seen: string
  trace_id?: string | null
}
export interface BackendIssuesResult {
  /** Absent on the fleet-wide list — there, each row names its own service. */
  service?: string
  window: { start: string; end: string }
  errors: BackendIssue[]
}
export interface BackendIssueFacetsResult {
  service: string
  facets: Record<string, FacetResult>
}
export interface BackendIssueTag {
  field: string
  values: FacetValue[]
}
/** One occurrence: a single errored span, with everything needed to jump to it. */
export interface BackendIssueEvent {
  timestamp: string
  trace_id: string
  span_id: string
  operation: string
  kind: string
  duration_nanos: string | null
  status_message: string
  host: string
  environment: string
}
/** `[epoch ms, occurrences]` — already in the shape the chart components read. */
export type IssuePoint = [number, number]
export interface BackendIssueDetail {
  service: string
  fingerprint: string
  error_type: string
  message: string
  first_seen: string
  last_seen: string
  occurrences: number
  traces: number
  operations: number
  series: IssuePoint[]
  tags: BackendIssueTag[]
  /** `null` when the producer set `status = ERROR` without recording an exception. */
  sample_stack: string | null
  events: BackendIssueEvent[]
}

// --- Service map (`GET /api/services/map`) ---
/**
 * One service in the topology. Counts are over ALL its spans in the window, not just the ones on
 * edges — a node's size should mean "how busy is this", not "how connected is this".
 */
export interface ServiceMapNode {
  service: string
  count: number
  rate: number
  error_count: number
  error_rate: number
  /** Spans with no parent — trace roots. Where work enters the system. */
  roots: number
  is_entry: boolean
}
/**
 * One directed call edge. Latency is the CALLEE's own span duration, not the caller's wall-clock:
 * the difference is network and queueing, and mixing the two would make the number mean neither.
 */
export interface ServiceMapEdge {
  from: string
  to: string
  count: number
  rate: number
  error_count: number
  error_rate: number
  /** Nanosecond strings (JS-safe). */
  p50: string
  p95: string
  p99: string
}
export interface ServiceMapResult {
  window: { start: string; end: string }
  /** The focused service, or `null` for the fleet map. */
  service: string | null
  nodes: ServiceMapNode[]
  edges: ServiceMapEdge[]
  /** Spans actually read. Lets a thin map be told apart from a truncated one. */
  scanned: number
  /** `true` when the scan cap bit — edges are then incomplete, and the UI must say so. */
  capped: boolean
}

// --- Deployments (`GET/POST /api/deployments`) ---
/**
 * One release.
 *
 * `source` is how the row was FIRST noticed, not what it now contains: a `detected` row that CI
 * later enriched keeps `detected`, and the presence of `commit_sha`/`author`/`message` is what says
 * CI got involved. `detected_at_ms` is the first sighting by whichever side saw it first, and is
 * never rewritten — a late CI post must not move a deploy marker away from the moment the graph
 * actually changed.
 */
export interface Deployment {
  id: string
  service: string
  environment: string
  tenant: string
  version: string
  /** Unix MILLISECONDS (this table stores ms, unlike the ns-string span windows). */
  detected_at_ms: number
  source: 'detected' | 'ci'
  commit_sha: string | null
  author: string | null
  message: string | null
  url: string | null
}
export interface DeploymentsResult {
  deployments: Deployment[]
}
// --- Per-service data usage (`GET /api/usage/services`) ---
/** One signal's contribution for one service. */
export interface ServiceUsageSignal {
  rows: number
  /** ESTIMATED bytes: this service's rows × the signal's mean bytes/row. Never measured. */
  bytes: number
}
export interface ServiceUsageRow {
  service: string
  signals: { logs: ServiceUsageSignal; traces: ServiceUsageSignal }
  rows_total: number
  bytes_total: number
}
export interface ServiceUsageSignalBasis {
  store_rows: number
  store_bytes: number
  bytes_per_row: number
  /**
   * `false` means this signal contributes to no row's total — NOT that it is empty. Metrics are
   * `false`: a metric series is keyed by name, and host metrics, RUM vitals and the service
   * rollup's own output do not belong to a service at all.
   */
  attributed: boolean
  capped: boolean
}
export interface ServiceUsageResult {
  window: { start: string; end: string }
  rows: ServiceUsageRow[]
  /** Always `true`. Kept on the wire so a reader cannot mistake these bytes for measured ones. */
  estimated: boolean
  signals: {
    logs: ServiceUsageSignalBasis
    traces: ServiceUsageSignalBasis
    metrics: ServiceUsageSignalBasis
  }
  notes: { metrics: string }
}

// --- SLOs, error budgets and burn rate (`/api/services/:service/slos*`) ---
export type SloKind = 'availability' | 'latency'
export interface Slo {
  id: string
  service: string
  environment: string
  /** `""` = the whole service; a name holds one endpoint to a tighter line than the service. */
  operation: string
  name: string
  kind: SloKind
  /** Fraction of good events promised, 0 < target < 1. 0.999 is three nines. */
  target: number
  /** Latency SLOs only; `0` on an availability SLO. */
  threshold_ms: number
  window_days: number
}
export interface SloInput {
  name: string
  environment?: string
  operation?: string
  kind: SloKind
  target: number
  threshold_ms?: number
  window_days: number
}
/** One burn-rate window pair. `firing` requires BOTH windows — see `docs/subsystems/services-apm.md`. */
export interface BurnWindow {
  long: string
  short: string
  long_rate: number
  short_rate: number
  threshold: number
  firing: boolean
}
export interface SloBudget {
  slo: Slo
  /** The window actually measured, epoch ms. When `partial`, less than the SLO's nominal window. */
  window_start_ms: number
  window_end_ms: number
  /** `true` when the rollup does not cover the whole SLO window. */
  partial: boolean
  total_events: number
  bad_events: number
  sli: number | null
  budget_events: number
  remaining_events: number
  remaining_fraction: number | null
  exhausts_in_secs: number | null
  fast: BurnWindow
  slow: BurnWindow
  /** Latency SLOs: the histogram edge the count was actually taken at. */
  measured_threshold_ms: number | null
}

export interface DeploymentFilter {
  service?: string
  environment?: string
  tenant?: string
  /** Epoch NANOSECOND strings, like every other window param. */
  start?: string
  end?: string
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
// One signal's `merge_once` health. Absent from the payload entirely on a node with no compactor
// attached (`PHOTON_DISABLE_COMPACTION`) — the UI treats that as "unknown", never as healthy.
export interface CompactionHealth {
  signal: string
  last_ok_ms: number | null
  consecutive_failures: number
  last_error: string | null
}
export interface StorageStats {
  signals: Record<string, SignalStorage>
  durable: { configured: boolean; pending: number; last_replicated_ms: number }
  compaction?: CompactionHealth[]
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
  q?: string
}
/**
 * One bucket of `/api/services/:service/timeseries`.
 *
 * This type was missing two thirds of what the route returns — the counts, the Apdex bands, the
 * entry-set basis — which meant the detail page could only draw rates and percentiles even though
 * everything else was already on the wire.
 */
export interface ServiceTimeseriesBucket {
  /** Bucket start, epoch ms. */
  ts: number
  source?: RedSource
  rate: number
  error_rate: number
  /** Requests served in the bucket: the series' ENTRY spans (ADR-0003). */
  count: number
  /** Every span in the bucket, all kinds — beside `count`, never instead of it. */
  span_count: number
  /** Which cascade rule produced `count`. The same value on every bucket of one series. */
  basis?: 'entry' | 'trace_root' | 'all_spans'
  error_count: number
  http_4xx: number
  http_5xx: number
  p50: string
  p90: string
  p99: string
  apdex: number | null
  satisfied: number
  tolerating: number
  frustrated: number
}
/**
 * One downstream call target: a database (`system` set) or an external peer (`system` null).
 *
 * NOT a Service — it has no page of its own, no health, and no spans it owns. `CONTEXT.md` is
 * explicit about the distinction, which is why these rows live apart from the service map's edges
 * even though `/services/:service/dependencies` draws both in one picture.
 */
export interface DepRow {
  name: string
  system: string | null
  count: number
  rate: number
  error_count: number
  error_rate: number
  /** Nanosecond strings (JS-safe). */
  p50: string
  p95: string
  p99: string
}
export interface ServiceDependencies {
  database: DepRow[]
  external: DepRow[]
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

// --- Tenants / Federation ---
export interface Tenant {
  name: string
  token: string // list responses redact this to `…last4`
  ui_url: string | null
  created_at: number
}
export interface TenantsResult {
  tenants: Tenant[]
}
export interface TenantSummary {
  name: string
  mode: string | null
  status: 'up' | 'stale' | 'down'
  last_seen_ms: number
  ingest_rows_per_sec: number
  open_incidents: number
  hot_bytes: number
  ui_url: string | null
  spark: [number, number][]
}
export interface FederationStatusSnapshot {
  mode: string
  endpoint: string
  last_push_ms: number
  last_error: string | null
  pushed: number
  dropped: number
  queued: number
}
export interface FederationStatusResult {
  enabled: boolean
  status: FederationStatusSnapshot | null
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
  // Disk and GPU are label-split (mountpoints, devices), so they cross as a PAIR: `*Util` is the
  // worst group, `*UtilAvg` the mean across groups, `*Groups` how many there were. The UI shows
  // both numbers — a lone max reads as a saturated host when only one group is hot.
  diskUtil: number | null
  diskUtilAvg: number | null
  diskGroups: number
  gpuUtil: number | null
  gpuUtilAvg: number | null
  gpuGroups: number
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
// One supervised process (keyed by `service.name`) running on a host, with its latest resource
// usage over the window. `cpuPct` is a percent (not a 0..1 ratio). Every numeric field is nullable —
// a process may report CPU without every other metric present in the window.
export interface InfraProcess {
  process: string
  cpuPct: number | null
  rssBytes: number | null
  fds: number | null
  threads: number | null
  restarts: number | null
  lastSeenNs: string
}
export interface InfraProcessesResult {
  processes: InfraProcess[]
}

// --- Alerts (webhook alert & notification engine) ---
// Field names/shapes mirror the alerts data model and its per-signal `condition` JSON exactly —
// see docs/subsystems/alerts.md.
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

// Should this error be surfaced to the user instead of silently falling back to the mock corpus?
//
// The mock fallback exists for ONE case: no backend at all (pure-frontend dev), where ky throws
// without a `.response` so `.status` stays undefined. A 4xx/5xx means the backend answered — it is
// reachable, and swapping in demo data would dress a real failure up as real data. That matters
// most for 5xx: the search handlers now 500 on a hard engine error (e.g. `ResourcesExhausted` from
// the shared query pool) instead of returning an empty page, and mocking those would replace a
// visible failure with a plausible-looking fake result set.
//
// 401 is deliberately NOT surfaced here — an expired session is the router auth guard's job, and
// throwing would turn it into a query error toast instead of a redirect to /login.
function isRealFailure(e: { status?: number }): boolean {
  return e.status != null && e.status !== 401
}

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

// Append a non-blank grammar `q` to a searchParams literal (shared by the RUM read methods).
function withQ(searchParams: Record<string, string>, q?: string): Record<string, string> {
  if (q && q.trim()) searchParams.q = q
  return searchParams
}

let mockUsers: UserInfo[] = [
  { username: 'demo', created_at: Date.now() - 90 * 86_400_000, role: 'admin' },
  { username: 'rina', created_at: Date.now() - 30 * 86_400_000, role: 'operator' },
  { username: 'bayu', created_at: Date.now() - 12 * 86_400_000, role: 'viewer' },
]

// Local mutable copy of `mockRetention` (imported `let` bindings from another ES module are
// live-read but not reassignable from here, so — like `mockUsers` above — the mutable state
// lives in this module and is only seeded from the mock.js default).
let mockRetentionState: Retention = { ...mockRetention }

// --- Docker fleet management: managed agents, enrollment, audit ---
// Field names mirror the control-plane tables in docs/subsystems/deploy.md. Timestamps here are
// epoch MILLISECONDS (control-plane rows, like alerts/deployments), not the telemetry ns strings.
export type AgentCapability = 'read' | 'lifecycle' | 'apply' | 'exec'
export type AgentStatus = 'online' | 'offline' | 'pending'
export interface DockerDisk {
  images_bytes: number
  volumes_bytes: number
  build_cache_bytes: number
  reclaimable_bytes: number
  total_bytes: number | null
  used_bytes: number | null
}
export interface DockerFacts {
  version: string
  api_version: string
  storage_driver: string
  os: string
  arch: string
  containers_running: number
  containers_total: number
  images: number
  disk: DockerDisk
}
export interface ManagedHost {
  id: string
  host: string
  site: string | null
  status: AgentStatus
  agent_version: string | null
  latest_agent_version: string | null
  // What the agent ADMITS to, reported on every sync — never assigned from here.
  capabilities: AgentCapability[]
  poll_secs: number
  last_seen_ms: number | null
  enrolled_ms: number
  docker: DockerFacts | null
  labels: Record<string, string>
}
export interface ManagedHostsResult {
  hosts: ManagedHost[]
}
export interface EnrollInput {
  host: string
  site?: string | null
  capabilities: AgentCapability[]
  poll_secs?: number
  labels?: Record<string, string>
}
export interface EnrollResult extends MutationResult {
  host?: ManagedHost
  // The per-host agent token, minted server-side and shown ONCE.
  token?: string
}
export interface AgentPatch {
  poll_secs?: number
  labels?: Record<string, string>
  site?: string | null
}
export type SshAuth = { type: 'password'; password: string } | { type: 'key'; private_key: string; passphrase?: string }
export interface SshBootstrapInput {
  addresses: string[]
  port: number
  username: string
  auth: SshAuth
  host_name?: string | null
  site?: string | null
  capabilities: AgentCapability[]
  poll_secs?: number
  labels?: Record<string, string>
  // Off by default: credentials are used once and discarded (R1.9.1).
  remember_credentials?: boolean
}
export type BootstrapStepKey = 'connect' | 'detect' | 'push' | 'configure' | 'start' | 'sync'
export interface BootstrapStep {
  key: BootstrapStepKey
  label: string
  status: 'pending' | 'running' | 'done' | 'failed' | 'skipped'
  detail?: string
}
export interface BootstrapHostProgress {
  address: string
  host: string | null
  status: 'running' | 'done' | 'failed'
  steps: BootstrapStep[]
  error: string | null
}
export interface BootstrapJob {
  id: string
  status: 'running' | 'done' | 'failed'
  started_ms: number
  credentials_kept: boolean
  hosts: BootstrapHostProgress[]
}
export interface BootstrapStartResult extends MutationResult {
  job?: BootstrapJob
}
export type AuditResultKind = 'ok' | 'failed' | 'denied' | 'pending'
export interface AuditTarget {
  host?: string
  container?: string
  image?: string
  volume?: string
  stack?: string
  rollout?: string
  template?: string
  version?: string
  registry?: string
  user?: string
}
export interface AuditEntry {
  id: string
  ts_ms: number
  actor: string
  role: Role
  // Dotted `noun.verb` — `container.restart`, `stack.apply`, `agent.enroll`, `rollout.promote`.
  action: string
  target: AuditTarget
  result: AuditResultKind
  detail: string | null
  // Digest of the rendered payload; the payload itself is never in the log (secrets, R5.9).
  payload_digest: string | null
}
export interface AuditFilter {
  since_ms?: number
  actor?: string
  action?: string
  host?: string
  result?: AuditResultKind
  q?: string
  limit?: number
}
export interface AuditResult {
  entries: AuditEntry[]
  total: number
}

// --- Docker fleet management: per-host inventory (read-only Docker) ---
export type ContainerState = 'running' | 'exited' | 'paused' | 'restarting' | 'created' | 'dead' | 'removing'
export type ContainerHealth = 'healthy' | 'unhealthy' | 'starting' | 'none'
export interface ContainerPort {
  host_ip: string | null
  host_port: number | null
  container_port: number
  protocol: 'tcp' | 'udp'
}
export interface ContainerMount {
  type: 'volume' | 'bind' | 'tmpfs'
  source: string
  destination: string
  rw: boolean
}
export interface Container {
  id: string
  name: string
  image: string
  image_id: string
  command: string
  state: ContainerState
  health: ContainerHealth
  // Docker's own words ("Up 3 days (healthy)", "Exited (1) 2 hours ago") — shown verbatim.
  status_text: string
  created_ms: number
  started_ms: number | null
  finished_ms: number | null
  exit_code: number | null
  restart_count: number
  restart_policy: string
  ports: ContainerPort[]
  mounts: ContainerMount[]
  networks: string[]
  labels: Record<string, string>
  // Names only — values may be secrets and never leave the host.
  env_keys: string[]
  // Values as the AGENT reports them: anything secret-looking is masked before it leaves the host.
  env: Record<string, string>
  // From `com.docker.compose.project` / `.service`, so a stack Photon deployed and one somebody
  // deployed by hand group the same way.
  compose_project: string | null
  compose_service: string | null
  managed_stack_id: string | null
  cpu_pct: number | null
  mem_bytes: number | null
  mem_limit_bytes: number | null
  net_rx_bps: number | null
  net_tx_bps: number | null
  block_read_bps: number | null
  block_write_bps: number | null
}
export interface ImageInfo {
  id: string
  repo_tags: string[]
  size_bytes: number
  created_ms: number
  used_by: number
  dangling: boolean
}
export interface VolumeInfo {
  name: string
  driver: string
  mountpoint: string
  size_bytes: number | null
  used_by: string[]
  created_ms: number
}
export interface NetworkInfo {
  id: string
  name: string
  driver: string
  scope: string
  internal: boolean
  containers: number
  subnet: string | null
}
export interface HostInventory {
  host: string
  agent_status: AgentStatus
  // When the agent last reported this. `stale` = older than two poll intervals (offline host).
  fetched_ms: number
  stale: boolean
  containers: Container[]
  images: ImageInfo[]
  volumes: VolumeInfo[]
  networks: NetworkInfo[]
  facts: DockerFacts | null
}
export interface ContainerLogLine {
  ts_ms: number
  stream: 'stdout' | 'stderr'
  text: string
}
export interface ContainerLogsQuery {
  tail?: number
  since_ms?: number
}
export interface ContainerLogsResult {
  lines: ContainerLogLine[]
  truncated: boolean
}
export interface ContainerStatPoint {
  t: number
  v: number | null
}
export interface ContainerStats {
  cpu_pct: ContainerStatPoint[]
  mem_bytes: ContainerStatPoint[]
  net_rx_bps: ContainerStatPoint[]
  net_tx_bps: ContainerStatPoint[]
  block_read_bps: ContainerStatPoint[]
  block_write_bps: ContainerStatPoint[]
  mem_limit_bytes: number | null
}
export interface DockerEvent {
  id: string
  ts_ms: number
  type: 'container' | 'image' | 'volume' | 'network'
  action: string
  actor: string
  attributes: Record<string, string>
}
export interface DockerEventsResult {
  events: DockerEvent[]
}

// --- Docker fleet management: one-shot Actions (lifecycle) ---
export type ActionKind = 'start' | 'stop' | 'restart' | 'kill' | 'pull' | 'recreate' | 'remove' | 'prune' | 'image_pull' | 'image_remove' | 'volume_remove'
export type ActionStatus = 'queued' | 'running' | 'done' | 'failed' | 'expired'
export interface ActionArgs {
  // remove: also delete anonymous volumes; recreate: pull first
  remove_volumes?: boolean
  pull?: boolean
  // prune: which categories
  images?: boolean
  containers?: boolean
  volumes?: boolean
  build_cache?: boolean
}
export interface ActionInput {
  kind: ActionKind
  container_id?: string
  // image_pull / image_remove: the image id or `repo:tag`.
  image?: string
  // volume_remove: the volume's name.
  volume?: string
  args?: ActionArgs
}
export interface ActionRecord {
  id: string
  host: string
  kind: ActionKind
  container_id: string | null
  container_name: string | null
  image: string | null
  volume?: string | null
  args: ActionArgs
  status: ActionStatus
  actor: string
  created_ms: number
  started_ms: number | null
  finished_ms: number | null
  // The lease: an action nobody picked up by then expires instead of firing late.
  expires_ms: number
  result: string | null
}
export interface ActionResult extends MutationResult {
  action?: ActionRecord
}
export interface ActionsResult {
  actions: ActionRecord[]
}
export interface PruneCategory {
  count: number
  bytes: number
}
export interface PrunePreview {
  host: string
  images: PruneCategory
  containers: PruneCategory
  volumes: PruneCategory
  build_cache: PruneCategory
}

// --- Docker fleet management: Stacks (compose projects under desired state) ---
export type StackStatus = 'declared' | 'rendered' | 'pending' | 'applying' | 'running' | 'drifted' | 'failed' | 'destroying' | 'gone'
export type StackSource = 'template' | 'compose' | 'adopted'
export interface StackFile {
  path: string
  content: string
}
export interface StackServiceState {
  name: string
  image: string
  containers: { id: string; name: string; state: ContainerState; health: ContainerHealth }[]
}
export interface DriftItem {
  kind: 'container_missing' | 'extra_container' | 'image_changed' | 'config_changed' | 'state_changed' | 'env_changed'
  service: string | null
  field: string | null
  desired: string | null
  observed: string | null
  since_ms: number
}
export interface DeployOptions {
  pull: 'missing' | 'always' | 'never'
  force_recreate: boolean
  recreate_volumes: boolean
}
export interface ComposeWarning {
  // Stable across edits of the same construct, so an acknowledgement sticks.
  id: string
  kind: 'privileged' | 'docker_sock' | 'host_network' | 'host_pid' | 'bind_mount_outside' | 'cap_add'
  severity: 'error' | 'warning'
  service: string | null
  line: number | null
  detail: string
}
export interface Stack {
  id: string
  name: string
  host: string
  site: string | null
  source: StackSource
  template_id: string | null
  template_name: string | null
  template_version: string | null
  status: StackStatus
  desired_generation: number
  applied_generation: number | null
  files: StackFile[]
  // Resolved non-secret variables; secret NAMES only.
  variables: Record<string, string>
  secrets: string[]
  deploy_options: DeployOptions
  services: StackServiceState[]
  drift: DriftItem[]
  // APM services this stack produced (service.name), for the observability tie-in.
  apm_services: string[]
  acknowledged_warnings: string[]
  last_error: string | null
  created_by: string
  created_ms: number
  updated_ms: number
  last_applied_ms: number | null
}
export interface StacksResult {
  stacks: Stack[]
}
export interface StackInput {
  name: string
  host: string
  files: StackFile[]
  deploy_options: DeployOptions
  acknowledged_warnings: string[]
  // Create and apply in one go (default), or just save.
  apply?: boolean
}
export interface StackResult extends MutationResult {
  stack?: Stack
}
// `accept` = take what runs as the new desired state (the deliberate alternative to reconcile).
export type StackActionKind = 'up' | 'down' | 'restart' | 'redeploy' | 'destroy' | 'reconcile' | 'accept'
export interface StackActionArgs {
  pull?: boolean
  volumes?: boolean
}
export interface StackValidation {
  ok: boolean
  warnings: ComposeWarning[]
  services: string[]
  missing_variables: string[]
  error: string | null
}
export interface DiscoveredProject {
  host: string
  name: string
  services: string[]
  containers: number
  running: number
  compose_file: string | null
  working_dir: string | null
}
export interface DiscoveredProjectsResult {
  projects: DiscoveredProject[]
}

// --- Docker fleet management: Templates (versioned, parameterized recipes) ---
export type VariableType = 'string' | 'number' | 'bool' | 'select' | 'port' | 'path' | 'secret' | 'generated'
export interface TemplateVariable {
  name: string
  label: string
  description?: string | null
  type: VariableType
  required: boolean
  default?: string | null
  // Fixed by the template author; shown, not editable (Portainer's `preset`).
  preset?: boolean
  options?: string[]
  pattern?: string | null
  group?: string | null
}
export interface TemplateRequires {
  arch?: string[]
  min_docker?: string | null
  min_ram_bytes?: number | null
  min_disk_bytes?: number | null
}
export interface TemplatePort {
  name: string
  container: number
  default_host: number
  protocol: 'tcp' | 'udp'
  description?: string | null
}
export interface TemplateVolume {
  name: string
  path: string
  description?: string | null
}
export interface TemplateHealth {
  kind: 'container_healthy' | 'uptime_monitor' | 'slo_burn'
  detail: string
  grace_secs: number
}
// The Photon block (R5.8): what the render injects into the deployed services.
export interface TemplateObservability {
  otlp_endpoint: boolean
  ingest_token: boolean
  service_name: string | null
  environment: boolean
}
export interface TemplateManifest {
  id: string
  version: string
  name: string
  description: string
  icon: string
  categories: string[]
  variables: TemplateVariable[]
  requires: TemplateRequires
  ports: TemplatePort[]
  volumes: TemplateVolume[]
  health: TemplateHealth | null
  observability: TemplateObservability
}
export interface TemplateVersionRef {
  id: string
  version: string
  created_ms: number
  created_by: string
  notes: string | null
  deployed_count: number
}
export interface TemplateVersion extends TemplateVersionRef {
  template_id: string
  manifest: TemplateManifest
  compose_text: string
}
export type TemplateSource = 'local' | 'registry' | 'portainer'
export interface Template {
  id: string
  name: string
  description: string
  icon: string
  categories: string[]
  source: TemplateSource
  registry_id: string | null
  latest_version: string
  versions: TemplateVersionRef[]
  updated_ms: number
  // Stacks deployed from any version of this template.
  stacks: number
}
export interface TemplatesResult {
  templates: Template[]
}
export interface TemplateVersionInput {
  template_id: string | null
  manifest: TemplateManifest
  compose_text: string
  notes?: string | null
}
export interface TemplateVersionResult extends MutationResult {
  version?: TemplateVersion
  template?: Template
}
export interface Registry {
  id: string
  name: string
  url: string
  kind: 'photon' | 'portainer_v2' | 'portainer_v3'
  last_sync_ms: number | null
  template_count: number
  status: 'ok' | 'error' | 'syncing'
  error: string | null
}
export interface RegistriesResult {
  registries: Registry[]
}
export interface RegistryInput {
  name: string
  url: string
  kind: Registry['kind']
}
export interface RegistryResult extends MutationResult {
  registry?: Registry
}
// A pull credential for a private image registry (a GitLab deploy token, a GHCR PAT, a Hub
// login). The token is write-only: never in this view. Distinct from `Registry`, a TEMPLATE
// catalog.
export interface RegistryCredential {
  id: string
  // The registry host as an image reference names it: `registry.gitlab.com`, `ghcr.io`, `docker.io`.
  server: string
  username: string
  label: string | null
  // `null` = every managed host.
  scope: TargetSelector | null
  created_ms: number
  created_by: string
  updated_ms: number
}
export interface RegistryCredentialsResult {
  credentials: RegistryCredential[]
}
// `secret` is required on create; on update, absent keeps the stored token and `scope: null`
// widens to every host.
export interface RegistryCredentialInput {
  server?: string
  username?: string
  secret?: string
  label?: string | null
  scope?: TargetSelector | null
}
export interface RegistryCredentialResult extends MutationResult {
  credential?: RegistryCredential
}
export interface ImportResult extends MutationResult {
  imported?: number
  skipped?: number
}
export interface PreflightCheck {
  key: string
  label: string
  status: 'pass' | 'warn' | 'fail' | 'skip'
  detail: string
}
export interface TemplateDeployInput {
  template_version_id: string
  host: string
  name: string
  variables: Record<string, string>
  // Secret values, sent once, never read back.
  secrets?: Record<string, string>
  deploy_options: DeployOptions
  acknowledged_warnings: string[]
  // Upgrade an existing stack in place (its id) rather than create one.
  stack_id?: string | null
}
export interface DryRunResult {
  ok: boolean
  host: string
  checks: PreflightCheck[]
  compose: string
  env: string
  // The compose the host runs now, when upgrading an existing stack — for the diff.
  previous_compose: string | null
  warnings: ComposeWarning[]
  secrets: string[]
  generated: string[]
  error: string | null
}

// --- Docker fleet management: Rollouts (staged application of a template version) ---
export type TargetSelector = { kind: 'hosts'; hosts: string[] } | { kind: 'site'; sites: string[] } | { kind: 'labels'; labels: Record<string, string> }
export interface RolloutStrategy {
  canary: number
  canary_is_percent: boolean
  // Stage pauses when this many targets in it have failed (R6.4).
  failure_threshold: number
  prepull: boolean
  // Automatic promotion past the gate — MAY, later; the UI carries the switch, off by default.
  auto_promote: boolean
  health_grace_secs: number
}
export type RolloutState = 'draft' | 'running' | 'paused_gate' | 'paused_threshold' | 'completed' | 'aborted' | 'rolled_back'
// `queued` = its stage has not been reached; `pending` = its stage is live but the host is not
// answering. Two facts, two words (the mistake `idle` once made on the Backend world).
export type TargetStatus = 'queued' | 'pending' | 'prepulling' | 'applying' | 'healthy' | 'failed' | 'drifted' | 'skipped'
export interface RolloutTarget {
  host: string
  site: string | null
  // 0 = not eligible (skipped at planning); 1 = canary; 2 = the rest.
  stage: number
  status: TargetStatus
  attempts: number
  last_error: string | null
  stack_id: string | null
  previous_version: string | null
  agent_status: AgentStatus
  updated_ms: number
}
export interface Rollout {
  id: string
  name: string
  template_id: string
  template_name: string
  template_version_id: string
  template_version: string
  previous_version: string | null
  stack_name: string
  selector: TargetSelector
  strategy: RolloutStrategy
  state: RolloutState
  stage: number
  stages: number
  targets: RolloutTarget[]
  variables: Record<string, string>
  created_by: string
  created_ms: number
  updated_ms: number
  started_ms: number | null
  finished_ms: number | null
}
export interface RolloutsResult {
  rollouts: Rollout[]
}
export interface RolloutInput {
  name: string
  template_version_id: string
  stack_name: string
  selector: TargetSelector
  strategy: RolloutStrategy
  variables: Record<string, string>
  secrets?: Record<string, string>
  deploy_options: DeployOptions
  // Every dangerous construct in the version's compose, acknowledged before it ships (R8.4).
  acknowledged_warnings: string[]
  start?: boolean
}
export interface RolloutResult extends MutationResult {
  rollout?: Rollout
}
export type RolloutActionKind = 'start' | 'promote' | 'pause' | 'resume' | 'abort' | 'rollback' | 'retry'

// --- Docker fleet management: exec (shell into a container; R3.4, admin + agent `exec`) ---
export interface ExecSession {
  id: string
  host: string
  container_id: string
  container_name: string
  shell: string
  opened_ms: number
}
export interface ExecSessionResult extends MutationResult {
  session?: ExecSession
}
export interface ExecOutput {
  lines: string[]
  exit_code: number | null
  // The session ended (an `exit`, or the agent closed it).
  closed: boolean
  // Output arrived after the line this call sent (a poll: during its wait). `lines` without it
  // are a previous command's late output; the answer is still coming.
  answered?: boolean
}

// ---------------------------------------------------------------------------
// Docker fleet management: availability. The subsystem's routes do not exist on a backend that
// predates it, and that backend answers an EMPTY 404 for them. Two honest outcomes exist for
// that, and which one applies is a deliberate switch, never a guess:
//   - demo mode (a dev build, `localStorage['photon-deploy-demo'] = '1'`, or no backend at all):
//     the mock corpus stands in, and the shell's "demo data" dot says so;
//   - otherwise: the request throws `DeployUnavailableError`, queries carry it as their error,
//     and the Deploy screens say the subsystem is not enabled — instead of painting a fake
//     Critical stack on a real install's landing page.
// A JSON-bodied 404 (`{ error }`) is always a real answer from a backend that HAS the subsystem.
// ---------------------------------------------------------------------------
export { DeployUnavailableError, isDeployUnavailable, setDeployDemo, DEPLOY_DEMO_KEY, DEPLOY_UNAVAILABLE_MESSAGE } from '@/lib/deploy/availability'

/** Demo data is in play when there is no backend at all, or the operator opted in. */
export function deployDemoEnabled(): boolean {
  return mockState.value || deployDemoFlag()
}

function emptyNotFound(e: ApiError): boolean {
  return e.status === 404 && !e.body?.error
}

// A deploy READ that failed: 400 is the caller's bug (throw); an empty 404 is the subsystem
// missing (demo → mock, else unavailable); any other answered status is a real failure (throw);
// no status at all is no backend (mock).
async function deployRead<T>(e: ApiError, mock: (m: DeployMocks) => T): Promise<T> {
  if (e.status === 400) throw e
  if (emptyNotFound(e)) {
    if (!deployDemoEnabled()) throw new DeployUnavailableError()
  } else if (e.status != null && e.status !== 401) throw e
  mockState.value = true
  return mock(await deployMocks())
}

// A deploy MUTATION that failed: refusals surface as `{ ok: false, error }`; an empty 404 is the
// subsystem missing; any other answered status is a real failure with its message; no status is
// no backend (mock).
async function deployMutation<T extends MutationResult>(e: ApiError, mock: (m: DeployMocks) => T): Promise<T> {
  if (isRefusal(e)) return { ok: false, error: roleError(e) } as T
  if (emptyNotFound(e)) {
    if (!deployDemoEnabled()) return { ok: false, error: DEPLOY_UNAVAILABLE_MESSAGE } as T
  } else if (e.status != null && e.status !== 401) return { ok: false, error: e.body?.error ?? `The server answered ${e.status}` } as T
  mockState.value = true
  return mock(await deployMocks())
}

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

  // Returns the signed-in role when the server sends one (a backend with roles answers login with
  // the session shape); a backend without roles sends none, and `auth.ts` reads that as admin.
  async login(username: string, password: string, opts: RequestOpts = {}): Promise<{ ok: boolean; role?: Role }> {
    try {
      const text = await http.post('login', { json: { username, password }, signal: opts.signal }).text()
      let role: Role | undefined
      try {
        const body = text ? (JSON.parse(text) as { role?: Role }) : {}
        role = body.role
      } catch {
        /* empty or non-JSON body — no role */
      }
      return { ok: true, role }
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
      return { authenticated: true, username: 'demo', needs_setup: false, role: 'admin' }
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
  async createUser(username: string, password: string, role: Role = 'viewer', opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.post('users', { json: { username, password, role }, signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 400 || e.status === 409) return { ok: false, error: e.body?.error }
      api.mock = true
      mockUsers = [...mockUsers, { username, created_at: Date.now(), role }]
      return { ok: true }
    }
  },

  // Change a user's role. 403 = the caller is not an admin; surfaced, never mocked over.
  async updateUserRole(username: string, role: Role, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.patch(`users/${encodeURIComponent(username)}`, { json: { role }, signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 405) return { ok: false, error: 'This server does not support roles yet' }
      if (e.status === 400 || e.status === 403 || e.status === 404) return { ok: false, error: e.body?.error ?? (e.status === 403 ? 'Only an admin can change roles' : undefined) }
      api.mock = true
      mockUsers = mockUsers.map((u) => (u.username === username ? { ...u, role } : u))
      return { ok: true }
    }
  },

  // Set a password. Your own needs `currentPassword`; someone else's needs the admin role. The
  // 400 (wrong current password / too short), 403 and 404 are surfaced, never mocked over.
  async changePassword(username: string, password: string, currentPassword?: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.put(`users/${encodeURIComponent(username)}/password`, {
        json: { password, current_password: currentPassword ?? null },
        signal: opts.signal,
      })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 400 || e.status === 403 || e.status === 404) {
        return { ok: false, error: e.body?.error ?? (e.status === 403 ? "Only an admin can change someone else's password" : undefined) }
      }
      api.mock = true
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
      if (isRealFailure(e)) throw e // bad query or a failed backend — surface it, don't mock
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
      if (isRealFailure(e)) throw e // bad query or a failed backend — surface it, don't mock
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
      if (isRealFailure(e)) throw e // bad query or a failed backend — surface it, don't mock
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

  // The whole Services index in one request. N per-row calls at a 15s poll would be unusable the
  // moment a fleet has thirty services, which is why this route exists at all.
  async servicesOverview(query: string, startNs: string, endNs: string, buckets: number, opts: RequestOpts = {}): Promise<ServicesOverview> {
    try {
      return await http
        .get('services/overview', {
          searchParams: { q: query, start: startNs, end: endNs, buckets },
          signal: opts.signal,
        })
        .json<ServicesOverview>()
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      const rows = (mockRed(query, startNs, endNs, 'service') as RedRow[]).map((r) => ({
        ...r,
        spark: [] as SparkPoint[],
        error_spark: [] as SparkPoint[],
      }))
      return { source: 'raw', window: { start: startNs, end: endNs, buckets }, sites: [], rows }
    }
  },

  async serviceOperations(service: string, query: string, startNs: string, endNs: string, buckets: number, opts: RequestOpts = {}): Promise<ServiceOperations> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/operations`, {
          searchParams: { q: query, start: startNs, end: endNs, buckets },
          signal: opts.signal,
        })
        .json<ServiceOperations>()
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      const rows = (mockRed(query, startNs, endNs, 'operation') as RedRow[]).map((r) => ({
        ...r,
        spark: [] as SparkPoint[],
        error_spark: [] as SparkPoint[],
      }))
      return { source: 'raw', window: { start: startNs, end: endNs, buckets }, rows }
    }
  },

  async serviceOperationHeatmap(service: string, operation: string, query: string, startNs: string, endNs: string, buckets: number, opts: RequestOpts = {}): Promise<LatencyHeatmap> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/operations/${encodeURIComponent(operation)}/heatmap`, {
          searchParams: { q: query, start: startNs, end: endNs, buckets },
          signal: opts.signal,
        })
        .json<LatencyHeatmap>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockLatencyHeatmap(startNs, endNs, buckets) as LatencyHeatmap
    }
  },

  // --- Backend Issues. `service` omitted = the fleet-wide list. ---

  async backendIssues(service: string | null, q: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<BackendIssuesResult> {
    const path = service ? `services/${encodeURIComponent(service)}/errors` : 'services/errors'
    try {
      return await http
        .get(path, { searchParams: { q, start: startNs, end: endNs }, signal: opts.signal })
        .json<BackendIssuesResult>()
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      return mockBackendIssues(service, q, startNs, endNs) as BackendIssuesResult
    }
  },

  async backendIssueFacets(service: string, q: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<BackendIssueFacetsResult> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/errors/facets`, {
          searchParams: { q, start: startNs, end: endNs },
          signal: opts.signal,
        })
        .json<BackendIssueFacetsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockBackendIssueFacets(service) as BackendIssueFacetsResult
    }
  },

  async backendIssueDetail(service: string, fingerprint: string, q: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<BackendIssueDetail> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/errors/${encodeURIComponent(fingerprint)}`, {
          searchParams: { q, start: startNs, end: endNs },
          signal: opts.signal,
        })
        .json<BackendIssueDetail>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockBackendIssueDetail(service, fingerprint, startNs, endNs) as BackendIssueDetail
    }
  },

  /**
   * The service topology. `service` narrows to edges touching it in BOTH directions; `null` is the
   * fleet map. Note it is a separate param, never folded into `q` — scoping the scan to one service
   * would delete the parent spans the join needs and the map would come back empty.
   */
  async deployments(filter: DeploymentFilter = {}, opts: RequestOpts = {}): Promise<DeploymentsResult> {
    const searchParams: Record<string, string> = {}
    for (const [k, v] of Object.entries(filter)) if (v) searchParams[k] = v
    try {
      return await http.get('deployments', { searchParams, signal: opts.signal }).json<DeploymentsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      // A 404 here means "this node has no deployments table", which is a real answer, not a
      // fallback case — but the mock path exists so the offline UI still has a timeline to draw.
      api.mock = true
      return mockDeployments(filter) as DeploymentsResult
    }
  },

  async serviceMap(service: string | null, q: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<ServiceMapResult> {
    try {
      return await http
        .get('services/map', {
          searchParams: { q, start: startNs, end: endNs, ...(service ? { service } : {}) },
          signal: opts.signal,
        })
        .json<ServiceMapResult>()
    } catch (e: any) {
      if (e.status === 400) throw e // bad query — surface it, don't mock
      api.mock = true
      return mockServiceMap(service, startNs, endNs) as ServiceMapResult
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

  async serviceTimeseries(service: string, { start, end, buckets, q }: ServiceTimeseriesRange = {}, opts: RequestOpts = {}): Promise<ServiceTimeseriesBucket[]> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/timeseries`, {
          searchParams: { start, end, buckets, ...(q ? { q } : {}) }, signal: opts.signal,
        })
        .json<ServiceTimeseriesBucket[]>()
    } catch {
      api.mock = true
      return mockServiceTimeseries(service, buckets, { start, end }) as ServiceTimeseriesBucket[]
    }
  },

  async serviceDependencies(service: string, { start, end, q }: { start?: string; end?: string; q?: string } = {}, opts: RequestOpts = {}): Promise<ServiceDependencies> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/dependencies`, {
          searchParams: { start, end, ...(q ? { q } : {}) }, signal: opts.signal,
        })
        .json<ServiceDependencies>()
    } catch {
      api.mock = true
      return { database: [], external: [] }
    }
  },

  async serviceUsage(startNs: string, endNs: string, limit = 50, opts: RequestOpts = {}): Promise<ServiceUsageResult> {
    try {
      return await http
        .get('usage/services', {
          searchParams: { start: startNs, end: endNs, limit: String(limit) },
          signal: opts.signal,
        })
        .json<ServiceUsageResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockServiceUsage(startNs, endNs, limit) as ServiceUsageResult
    }
  },

  async slos(service: string, opts: RequestOpts = {}): Promise<{ slos: Slo[] }> {
    try {
      return await http.get(`services/${encodeURIComponent(service)}/slos`, { signal: opts.signal }).json<{ slos: Slo[] }>()
    } catch {
      api.mock = true
      return { slos: mockSlos(service) as Slo[] }
    }
  },

  async createSlo(service: string, input: SloInput, opts: RequestOpts = {}): Promise<{ slo: Slo }> {
    // No mock fallback and no swallowed 400: a create that silently "succeeded" against nothing is
    // worse than an error, because the user then believes the objective exists.
    return await http
      .post(`services/${encodeURIComponent(service)}/slos`, { json: input, signal: opts.signal })
      .json<{ slo: Slo }>()
  },

  async updateSlo(service: string, id: string, input: SloInput, opts: RequestOpts = {}): Promise<{ slo: Slo }> {
    return await http
      .patch(`services/${encodeURIComponent(service)}/slos/${encodeURIComponent(id)}`, {
        json: input, signal: opts.signal,
      })
      .json<{ slo: Slo }>()
  },

  async deleteSlo(service: string, id: string, opts: RequestOpts = {}): Promise<void> {
    await http.delete(`services/${encodeURIComponent(service)}/slos/${encodeURIComponent(id)}`, { signal: opts.signal })
  },

  async sloBudget(service: string, id: string, opts: RequestOpts = {}): Promise<SloBudget> {
    try {
      return await http
        .get(`services/${encodeURIComponent(service)}/slos/${encodeURIComponent(id)}/budget`, { signal: opts.signal })
        .json<SloBudget>()
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) throw e
      api.mock = true
      return mockSloBudget(service, id) as SloBudget
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

  // With `tenant` set the server returns names-only rows derived from mirrored data — only `name`
  // is real; the manage-only fields are absent (the tenant lens hides every manage affordance).
  async rumApps(opts: RequestOpts = {}, tenant?: string): Promise<RumAppsResult> {
    try {
      return await http
        .get('rum/apps', { searchParams: tenant ? { tenant } : {}, signal: opts.signal })
        .json<RumAppsResult>()
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

  // --- Tenants / Federation ---

  async tenants(opts: RequestOpts = {}): Promise<TenantsResult> {
    try {
      return await http.get('tenants', { signal: opts.signal }).json<TenantsResult>()
    } catch {
      api.mock = true
      return mockTenants()
    }
  },

  async tenantsSummary(opts: RequestOpts = {}): Promise<TenantSummary[]> {
    try {
      return await http.get('tenants/summary', { signal: opts.signal }).json<TenantSummary[]>()
    } catch {
      api.mock = true
      return mockTenantsSummary()
    }
  },

  async createTenant(name: string, uiUrl?: string | null, opts: RequestOpts = {}): Promise<MutationResult & { token?: string }> {
    try {
      const res = await http.post('tenants', { json: { name, ui_url: uiUrl ?? null }, signal: opts.signal }).json<Tenant>()
      return { ok: true, token: res.token }
    } catch (e: any) {
      if (e.status === 400 || e.status === 409) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockCreateTenant(name, uiUrl)
    }
  },

  async updateTenant(name: string, uiUrl: string | null, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.patch(`tenants/${encodeURIComponent(name)}`, { json: { ui_url: uiUrl }, signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 400 || e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockUpdateTenant(name, uiUrl)
    }
  },

  async rotateTenantToken(name: string, opts: RequestOpts = {}): Promise<MutationResult & { token?: string }> {
    try {
      const res = await http.post(`tenants/${encodeURIComponent(name)}/rotate-token`, { signal: opts.signal }).json<{ token: string }>()
      return { ok: true, token: res.token }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockRotateTenantToken(name)
    }
  },

  async deleteTenant(name: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`tenants/${encodeURIComponent(name)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      if (e.status === 404) return { ok: false, error: e.body?.error }
      api.mock = true
      return mockDeleteTenant(name)
    }
  },

  async federationStatus(opts: RequestOpts = {}): Promise<FederationStatusResult> {
    try {
      return await http.get('federation/status', { signal: opts.signal }).json<FederationStatusResult>()
    } catch {
      api.mock = true
      return mockFederationStatus()
    }
  },

  async rumVitals(app: string, startNs: string, endNs: string, opts: RequestOpts = {}, q?: string): Promise<RumVitalsResult> {
    try {
      const searchParams = withQ({ app, start: startNs, end: endNs }, q)
      return await http
        .get('rum/vitals', { searchParams, signal: opts.signal })
        .json<RumVitalsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumVitals(app) as RumVitalsResult
    }
  },

  async rumBreakdown(app: string, dimension: string, startNs: string, endNs: string, opts: RequestOpts = {}, q?: string): Promise<RumBreakdownResult> {
    try {
      const searchParams = withQ({ app, dimension, start: startNs, end: endNs }, q)
      return await http
        .get('rum/vitals/breakdown', { searchParams, signal: opts.signal })
        .json<RumBreakdownResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumBreakdown(app, dimension) as RumBreakdownResult
    }
  },

  async rumPages(app: string, startNs: string, endNs: string, opts: RequestOpts = {}, q?: string): Promise<RumPagesResult> {
    try {
      const searchParams = withQ({ app, start: startNs, end: endNs }, q)
      return await http
        .get('rum/pages', { searchParams, signal: opts.signal })
        .json<RumPagesResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumPages(app) as RumPagesResult
    }
  },

  async rumPageDetail(app: string, route: string, startNs: string, endNs: string, opts: RequestOpts = {}, q?: string): Promise<RumPageDetailResult> {
    try {
      const searchParams = withQ({ app, route, start: startNs, end: endNs }, q)
      return await http
        .get('rum/pages/detail', { searchParams, signal: opts.signal })
        .json<RumPageDetailResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumPageDetail(app, route) as RumPageDetailResult
    }
  },

  async rumErrors(app: string, startNs: string, endNs: string, opts: RequestOpts = {}, q?: string): Promise<RumErrorsResult> {
    try {
      const searchParams = withQ({ app, start: startNs, end: endNs }, q)
      return await http.get('rum/errors', { searchParams, signal: opts.signal }).json<RumErrorsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumErrors(app, q) as RumErrorsResult
    }
  },

  async rumErrorFacets(app: string, q: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<RumErrorFacetsResult> {
    try {
      const searchParams = withQ({ app, start: startNs, end: endNs }, q)
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
    q?: string,
  ): Promise<RumErrorDetailResult> {
    try {
      const searchParams = withQ({ app, start: startNs, end: endNs }, q)
      return await http
        .get(`rum/errors/${encodeURIComponent(fingerprint)}`, { searchParams, signal: opts.signal })
        .json<RumErrorDetailResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockRumErrorDetail(app, fingerprint) as RumErrorDetailResult
    }
  },

  // --- Infrastructure (host/GPU resource monitoring) ---

  async infraHosts(startNs: string, endNs: string, q?: string, opts: RequestOpts = {}): Promise<InfraHostsResult> {
    try {
      return await http
        .get('infra/hosts', { searchParams: { start: startNs, end: endNs, ...(q ? { q } : {}) }, signal: opts.signal })
        .json<InfraHostsResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockInfraHosts() as InfraHostsResult
    }
  },

  async infraHost(host: string, startNs: string, endNs: string, q?: string, opts: RequestOpts = {}): Promise<InfraHostDetail> {
    try {
      return await http
        .get(`infra/hosts/${encodeURIComponent(host)}`, { searchParams: { start: startNs, end: endNs, ...(q ? { q } : {}) }, signal: opts.signal })
        .json<InfraHostDetail>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockInfraHost(host) as InfraHostDetail
    }
  },

  async infraHostProcesses(host: string, startNs: string, endNs: string, q?: string, opts: RequestOpts = {}): Promise<InfraProcessesResult> {
    try {
      return await http
        .get(`infra/hosts/${encodeURIComponent(host)}/processes`, { searchParams: { start: startNs, end: endNs, ...(q ? { q } : {}) }, signal: opts.signal })
        .json<InfraProcessesResult>()
    } catch (e: any) {
      if (e.status === 400) throw e
      api.mock = true
      return mockInfraHostProcesses(host) as InfraProcessesResult
    }
  },

  async infraHostSeries(
    host: string,
    resource: string,
    startNs: string,
    endNs: string,
    q?: string,
    opts: RequestOpts = {},
  ): Promise<InfraSeriesResult> {
    try {
      return await http
        .get(`infra/hosts/${encodeURIComponent(host)}/timeseries`, {
          searchParams: { resource, start: startNs, end: endNs, ...(q ? { q } : {}) },
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

  // --- Docker fleet management: managed agents, enrollment, audit ---
  // Reads fall back to the deploy mock corpus on anything but a 400 (the same rule the infra
  // methods use, so a backend that predates this subsystem — which 404s these routes — still gets
  // a working UI over demo data). Mutations surface 400/403/404/409 as `{ ok: false, error }`;
  // a 403 means the caller's ROLE was refused, and the message says so.

  async agents(opts: RequestOpts = {}): Promise<ManagedHostsResult> {
    try {
      return await http.get('agents', { signal: opts.signal }).json<ManagedHostsResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockManagedHosts())
    }
  },

  // `null` = this host has no managed agent (the /infra/:host Agent tab's enroll prompt).
  async agent(host: string, opts: RequestOpts = {}): Promise<ManagedHost | null> {
    try {
      return await http.get(`agents/${encodeURIComponent(host)}`, { signal: opts.signal }).json<ManagedHost>()
    } catch (e: any) {
      if (e.status === 400) throw e
      // A backend WITH the subsystem answers an unenrolled host with a JSON 404 (`{ error }`); a
      // backend WITHOUT it 404s the route with an empty body. Only the first is a real "not
      // enrolled" — the second falls through to the demo corpus like every other read here.
      if (e.status === 404 && e.body?.error) return null
      return deployRead(e, (m) => m.mockManagedHost(host))
    }
  },

  async enrollAgent(input: EnrollInput, opts: RequestOpts = {}): Promise<EnrollResult> {
    try {
      return await http.post('agents', { json: input, signal: opts.signal }).json<EnrollResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockEnrollHost(input))
    }
  },

  async updateAgent(host: string, patch: AgentPatch, opts: RequestOpts = {}): Promise<MutationResult & { host?: ManagedHost }> {
    try {
      return await http.patch(`agents/${encodeURIComponent(host)}`, { json: patch, signal: opts.signal }).json<MutationResult & { host?: ManagedHost }>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockUpdateAgent(host, patch))
    }
  },

  async rotateAgentToken(host: string, opts: RequestOpts = {}): Promise<MutationResult & { token?: string }> {
    try {
      return await http.post(`agents/${encodeURIComponent(host)}/rotate-key`, { signal: opts.signal }).json<MutationResult & { token?: string }>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockRotateAgentToken(host))
    }
  },

  async deleteAgent(host: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`agents/${encodeURIComponent(host)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockDeleteAgent(host))
    }
  },

  // Ask the server to advertise its latest agent version to this host on its next sync (R1.10).
  async requestAgentUpgrade(host: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.post(`agents/${encodeURIComponent(host)}/upgrade`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockRequestAgentUpgrade(host))
    }
  },

  async startBootstrap(input: SshBootstrapInput, opts: RequestOpts = {}): Promise<BootstrapStartResult> {
    try {
      return await http.post('agents/bootstrap', { json: input, signal: opts.signal }).json<BootstrapStartResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockStartBootstrap(input))
    }
  },

  async bootstrapJob(id: string, opts: RequestOpts = {}): Promise<BootstrapJob | null> {
    try {
      return await http.get(`agents/bootstrap/${encodeURIComponent(id)}`, { signal: opts.signal }).json<BootstrapJob>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockBootstrapJob(id))
    }
  },

  async audit(filter: AuditFilter = {}, opts: RequestOpts = {}): Promise<AuditResult> {
    const searchParams: Record<string, string | number> = {}
    if (filter.since_ms != null) searchParams.since = filter.since_ms
    if (filter.actor) searchParams.actor = filter.actor
    if (filter.action) searchParams.action = filter.action
    if (filter.host) searchParams.host = filter.host
    if (filter.result) searchParams.result = filter.result
    if (filter.q) searchParams.q = filter.q
    if (filter.limit) searchParams.limit = filter.limit
    try {
      return await http.get('audit', { searchParams, signal: opts.signal }).json<AuditResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockAudit(filter))
    }
  },

  // --- Docker fleet management: read-only inventory ---

  async hostInventory(host: string, opts: RequestOpts = {}): Promise<HostInventory | null> {
    try {
      return await http.get(`agents/${encodeURIComponent(host)}/inventory`, { signal: opts.signal }).json<HostInventory>()
    } catch (e: any) {
      if (e.status === 400) throw e
      if (e.status === 404 && e.body?.error) return null
      return deployRead(e, (m) => m.mockHostInventoryWithActions(host))
    }
  },

  async containerLogs(host: string, id: string, q: ContainerLogsQuery = {}, opts: RequestOpts = {}): Promise<ContainerLogsResult> {
    const searchParams: Record<string, string | number> = {}
    if (q.tail != null) searchParams.tail = q.tail
    if (q.since_ms != null) searchParams.since = q.since_ms
    try {
      return await http
        .get(`agents/${encodeURIComponent(host)}/containers/${encodeURIComponent(id)}/logs`, { searchParams, signal: opts.signal })
        .json<ContainerLogsResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockContainerLogs(host, id, q))
    }
  },

  async containerStats(host: string, id: string, startNs: string, endNs: string, opts: RequestOpts = {}): Promise<ContainerStats> {
    try {
      return await http
        .get(`agents/${encodeURIComponent(host)}/containers/${encodeURIComponent(id)}/stats`, { searchParams: { start: startNs, end: endNs }, signal: opts.signal })
        .json<ContainerStats>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockContainerStats(host, id, startNs, endNs))
    }
  },

  async dockerEvents(host: string, limit = 50, opts: RequestOpts = {}): Promise<DockerEventsResult> {
    try {
      return await http.get(`agents/${encodeURIComponent(host)}/events`, { searchParams: { limit }, signal: opts.signal }).json<DockerEventsResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockDockerEvents(host, limit))
    }
  },

  // --- Docker fleet management: one-shot actions ---

  async createAction(host: string, input: ActionInput, opts: RequestOpts = {}): Promise<ActionResult> {
    try {
      return await http.post(`agents/${encodeURIComponent(host)}/actions`, { json: input, signal: opts.signal }).json<ActionResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockCreateAction(host, input))
    }
  },

  async hostActions(host: string, limit = 20, opts: RequestOpts = {}): Promise<ActionsResult> {
    try {
      return await http.get(`agents/${encodeURIComponent(host)}/actions`, { searchParams: { limit }, signal: opts.signal }).json<ActionsResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockHostActions(host, limit))
    }
  },

  async prunePreview(host: string, opts: RequestOpts = {}): Promise<PrunePreview> {
    try {
      return await http.get(`agents/${encodeURIComponent(host)}/prune/preview`, { signal: opts.signal }).json<PrunePreview>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockPrunePreview(host))
    }
  },

  // --- Docker fleet management: Stacks ---

  async stacks(opts: RequestOpts = {}): Promise<StacksResult> {
    try {
      return await http.get('stacks', { signal: opts.signal }).json<StacksResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockStacks())
    }
  },

  async stack(id: string, opts: RequestOpts = {}): Promise<Stack | null> {
    try {
      return await http.get(`stacks/${encodeURIComponent(id)}`, { signal: opts.signal }).json<Stack>()
    } catch (e: any) {
      if (e.status === 400) throw e
      if (e.status === 404 && e.body?.error) return null
      return deployRead(e, (m) => m.mockStack(id))
    }
  },

  async createStack(input: StackInput, opts: RequestOpts = {}): Promise<StackResult> {
    try {
      return await http.post('stacks', { json: input, signal: opts.signal }).json<StackResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockCreateStack(input))
    }
  },

  // Save edited files as a new desired generation (does not apply by itself).
  async updateStackFiles(id: string, files: StackFile[], acknowledged: string[], deployOptions?: DeployOptions, opts: RequestOpts = {}): Promise<StackResult> {
    try {
      return await http.patch(`stacks/${encodeURIComponent(id)}`, { json: { files, acknowledged_warnings: acknowledged, deploy_options: deployOptions }, signal: opts.signal }).json<StackResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockUpdateStackFiles(id, files, acknowledged, deployOptions))
    }
  },

  async stackAction(id: string, kind: StackActionKind, args: StackActionArgs = {}, opts: RequestOpts = {}): Promise<StackResult> {
    try {
      return await http.post(`stacks/${encodeURIComponent(id)}/${kind}`, { json: args, signal: opts.signal }).json<StackResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockStackAction(id, kind, args))
    }
  },

  // Server-side render + validate without applying (R5.6 for a plain compose stack).
  async validateCompose(files: StackFile[], opts: RequestOpts = {}): Promise<StackValidation> {
    try {
      return await http.post('stacks/validate', { json: { files }, signal: opts.signal }).json<StackValidation>()
    } catch (e: any) {
      if (e.status === 400 && e.body?.error) return { ok: false, warnings: [], services: [], missing_variables: [], error: e.body.error }
      return deployRead(e, (m) => m.mockValidateCompose(files))
    }
  },

  async discoverProjects(host: string, opts: RequestOpts = {}): Promise<DiscoveredProjectsResult> {
    try {
      return await http.get(`agents/${encodeURIComponent(host)}/compose-projects`, { signal: opts.signal }).json<DiscoveredProjectsResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockDiscoverProjects(host))
    }
  },

  async adoptProject(host: string, project: string, opts: RequestOpts = {}): Promise<StackResult> {
    try {
      return await http.post('stacks/adopt', { json: { host, project }, signal: opts.signal }).json<StackResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockAdoptProject(host, project))
    }
  },

  // --- Docker fleet management: Templates & registries ---

  async templates(opts: RequestOpts = {}): Promise<TemplatesResult> {
    try {
      return await http.get('templates', { signal: opts.signal }).json<TemplatesResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockTemplates())
    }
  },

  async template(id: string, opts: RequestOpts = {}): Promise<Template | null> {
    try {
      return await http.get(`templates/${encodeURIComponent(id)}`, { signal: opts.signal }).json<Template>()
    } catch (e: any) {
      if (e.status === 400) throw e
      if (e.status === 404 && e.body?.error) return null
      return deployRead(e, (m) => m.mockTemplate(id))
    }
  },

  async templateVersion(id: string, opts: RequestOpts = {}): Promise<TemplateVersion | null> {
    try {
      return await http.get(`templates/versions/${encodeURIComponent(id)}`, { signal: opts.signal }).json<TemplateVersion>()
    } catch (e: any) {
      if (e.status === 400) throw e
      if (e.status === 404 && e.body?.error) return null
      return deployRead(e, (m) => m.mockTemplateVersion(id))
    }
  },

  // Publishing is always a NEW immutable version (R5.4); there is no edit-in-place route.
  async publishTemplateVersion(input: TemplateVersionInput, opts: RequestOpts = {}): Promise<TemplateVersionResult> {
    try {
      return await http.post('templates/versions', { json: input, signal: opts.signal }).json<TemplateVersionResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockPublishTemplateVersion(input))
    }
  },

  async registries(opts: RequestOpts = {}): Promise<RegistriesResult> {
    try {
      return await http.get('registries', { signal: opts.signal }).json<RegistriesResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockRegistries())
    }
  },

  async addRegistry(input: RegistryInput, opts: RequestOpts = {}): Promise<RegistryResult> {
    try {
      return await http.post('registries', { json: input, signal: opts.signal }).json<RegistryResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockAddRegistry(input))
    }
  },

  async syncRegistry(id: string, opts: RequestOpts = {}): Promise<RegistryResult> {
    try {
      return await http.post(`registries/${encodeURIComponent(id)}/sync`, { signal: opts.signal }).json<RegistryResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockSyncRegistry(id))
    }
  },

  async deleteRegistry(id: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`registries/${encodeURIComponent(id)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockDeleteRegistry(id))
    }
  },

  // Image registry credentials: what a host pulls private images with.
  async imageRegistries(opts: RequestOpts = {}): Promise<RegistryCredentialsResult> {
    try {
      return await http.get('image-registries', { signal: opts.signal }).json<RegistryCredentialsResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockImageRegistries())
    }
  },

  async addImageRegistry(input: RegistryCredentialInput, opts: RequestOpts = {}): Promise<RegistryCredentialResult> {
    try {
      return await http.post('image-registries', { json: input, signal: opts.signal }).json<RegistryCredentialResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockAddImageRegistry(input))
    }
  },

  async updateImageRegistry(id: string, input: RegistryCredentialInput, opts: RequestOpts = {}): Promise<RegistryCredentialResult> {
    try {
      return await http.patch(`image-registries/${encodeURIComponent(id)}`, { json: input, signal: opts.signal }).json<RegistryCredentialResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockUpdateImageRegistry(id, input))
    }
  },

  async deleteImageRegistry(id: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`image-registries/${encodeURIComponent(id)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockDeleteImageRegistry(id))
    }
  },

  // Portainer v2/v3 app-template JSON → local templates (R5.7).
  async importPortainer(json: string, sourceName: string, opts: RequestOpts = {}): Promise<ImportResult> {
    try {
      return await http.post('templates/import', { json: { format: 'portainer', source: sourceName, json }, signal: opts.signal }).json<ImportResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockImportPortainer(json, sourceName))
    }
  },

  // Render + validate against the target host's facts, without applying (R5.6).
  async dryRun(input: TemplateDeployInput, opts: RequestOpts = {}): Promise<DryRunResult> {
    try {
      return await http.post('templates/dry-run', { json: input, signal: opts.signal }).json<DryRunResult>()
    } catch (e: any) {
      if (e.status === 400 && e.body?.error) return { ok: false, host: input.host, checks: [], compose: '', env: '', previous_compose: null, warnings: [], secrets: [], generated: [], error: e.body.error }
      return deployRead(e, (m) => m.mockDryRun(input))
    }
  },

  async deployTemplate(input: TemplateDeployInput, opts: RequestOpts = {}): Promise<StackResult> {
    try {
      return await http.post('templates/deploy', { json: input, signal: opts.signal }).json<StackResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockDeployTemplate(input))
    }
  },

  // --- Docker fleet management: Rollouts ---

  async rollouts(opts: RequestOpts = {}): Promise<RolloutsResult> {
    try {
      return await http.get('rollouts', { signal: opts.signal }).json<RolloutsResult>()
    } catch (e: any) {
      return deployRead(e, (m) => m.mockRollouts())
    }
  },

  async rollout(id: string, opts: RequestOpts = {}): Promise<Rollout | null> {
    try {
      return await http.get(`rollouts/${encodeURIComponent(id)}`, { signal: opts.signal }).json<Rollout>()
    } catch (e: any) {
      if (e.status === 400) throw e
      if (e.status === 404 && e.body?.error) return null
      return deployRead(e, (m) => m.mockRollout(id))
    }
  },

  async createRollout(input: RolloutInput, opts: RequestOpts = {}): Promise<RolloutResult> {
    try {
      return await http.post('rollouts', { json: input, signal: opts.signal }).json<RolloutResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockCreateRollout(input))
    }
  },

  async rolloutAction(id: string, kind: RolloutActionKind, opts: RequestOpts = {}): Promise<RolloutResult> {
    try {
      return await http.post(`rollouts/${encodeURIComponent(id)}/${kind}`, { signal: opts.signal }).json<RolloutResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockRolloutAction(id, kind))
    }
  },

  // --- Docker fleet management: exec sessions ---
  // The agent opens `POST /api/agent/stream/:session` outbound; the UI only ever talks to the
  // server. Every session is audited (who, which container, how long, how many commands).

  async openExec(host: string, containerId: string, opts: RequestOpts = {}): Promise<ExecSessionResult> {
    try {
      return await http.post(`agents/${encodeURIComponent(host)}/containers/${encodeURIComponent(containerId)}/exec`, { signal: opts.signal }).json<ExecSessionResult>()
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockOpenExec(host, containerId))
    }
  },

  async execSend(sessionId: string, line: string, opts: RequestOpts = {}): Promise<ExecOutput> {
    try {
      return await http.post(`exec/${encodeURIComponent(sessionId)}/input`, { json: { line }, signal: opts.signal }).json<ExecOutput>()
    } catch (e: any) {
      // A JSON-bodied 400/404 is the server closing the session; an empty 404 is a backend without
      // the route yet (mock).
      if ((e.status === 400 || e.status === 404) && e.body?.error) return { lines: [e.body.error], exit_code: null, closed: true }
      return deployRead(e, (m) => m.mockExecSend(sessionId, line))
    }
  },

  async closeExec(sessionId: string, opts: RequestOpts = {}): Promise<MutationResult> {
    try {
      await http.delete(`exec/${encodeURIComponent(sessionId)}`, { signal: opts.signal })
      return { ok: true }
    } catch (e: any) {
      return deployMutation(e, (m) => m.mockCloseExec(sessionId))
    }
  },
}

// Did the backend REFUSE this mutation (as opposed to not having the route at all)? 400/403/409
// are always real; a 404 is real only when it carries a JSON `error` body — an axum fallback for a
// route that does not exist answers with an empty 404, and that is the "no subsystem yet" case
// the mock corpus stands in for.
function isRefusal(e: ApiError): boolean {
  return e.status === 400 || e.status === 403 || e.status === 409 || (e.status === 404 && !!e.body?.error)
}

// The `{ ok: false, error }` message for a refused mutation. A 403 is the role check saying no,
// and the UI should say that in those words rather than echo an empty body.
function roleError(e: ApiError): string | undefined {
  if (e.status === 403) return e.body?.error ?? 'Your role is not allowed to do this'
  return e.body?.error
}
