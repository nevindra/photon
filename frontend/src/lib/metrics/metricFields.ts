// UI metadata for the metrics query builder: the grammar-filter field catalog (per selected
// metric), the aggregation dropdown options, and which metric types chart (all five, as of
// Task 6) and which disable group-by (summary only, since its quantiles are precomputed).
// Aggregation semantics mirror the backend (photon-core/src/metric_agg.rs default_agg table).

export interface MetricExampleQuery {
  text: string
  description: string
}

export const METRIC_EXAMPLE_QUERIES: MetricExampleQuery[] = [
  { text: 'service:checkout', description: 'One service' },
  { text: 'service:checkout,cart', description: 'Either service (OR-list)' },
  { text: '-http.method:GET', description: 'Exclude a value' },
  { text: 'http.status_code>=500', description: 'Numeric compare on an attribute' },
]

// The SearchBar catalog shape: { name, description, kind: 'match'|'compare', values?: string[]|'services' }.
// Same shape as `lib/logs/fields.ts`'s `FieldDescriptor` / `lib/traces/spanFields.ts`'s
// `SpanFieldDescriptor` — each module keeps its own interface (no shared base today).
export interface MetricFieldDescriptor {
  name: string
  description: string
  kind: 'match' | 'compare'
  values?: string[] | 'services'
}

export function buildMetricCatalog(
  attributeKeys: string[] = [],
  _services: string[] = [],
): MetricFieldDescriptor[] {
  const keys = attributeKeys.length ? attributeKeys : ['service']
  return keys.map((name) =>
    name === 'service' || name === 'service.name'
      ? { name: 'service', description: 'Service name', kind: 'match', values: 'services' }
      : { name, description: `Label ${name}`, kind: 'match' },
  )
}

// The fixed aggregation vocabulary accepted by the query engine (photon-core/src/metric_agg.rs).
export type AggName =
  | 'rate' | 'increase' | 'sum' | 'avg' | 'min' | 'max' | 'last' | 'count'
  | 'p50' | 'p90' | 'p99' | 'median'

export const AGG_OPTIONS: Record<AggName, string> = {
  rate: 'Rate / sec',
  increase: 'Increase',
  sum: 'Sum',
  avg: 'Average',
  min: 'Min',
  max: 'Max',
  last: 'Last',
  count: 'Count',
  p50: 'p50',
  p90: 'p90',
  p99: 'p99',
  median: 'Median',
}

// Aggregations offered per type, smart default first. All five metric types chart now
// (see isChartable below), so the histogram/exp_histogram/summary quantile lists here are
// live, not just future-proofing. `type` is deliberately loose (not a metric-type union): callers
// pass the raw `metric_type` string off the wire, which may be unknown/empty (falls to default).
export function aggOptionsForType(type: string, isMonotonic: boolean | null | undefined): AggName[] {
  switch (type) {
    case 'gauge':
      return ['avg', 'min', 'max', 'last', 'sum']
    case 'sum':
      return isMonotonic ? ['rate', 'increase', 'sum'] : ['sum', 'rate', 'increase']
    case 'histogram':
    case 'exp_histogram':
      return ['p99', 'p90', 'p50', 'count', 'sum', 'avg']
    case 'summary':
      return ['median', 'p90', 'p99']
    default:
      return ['avg']
  }
}

export function defaultAggForType(type: string, isMonotonic: boolean | null | undefined): AggName {
  return aggOptionsForType(type, isMonotonic)[0] ?? 'avg'
}

// `agg` is loosely typed (string) since callers may echo back an arbitrary/unrecognized value
// (e.g. a server-reported `default_agg`) and should still get a graceful fallback label.
export function aggLabel(agg: string): string {
  return AGG_OPTIONS[agg as AggName] ?? agg
}

// All five metric types chart end-to-end: gauge/sum via the value column, histogram/exp_histogram
// via interpolated quantiles, summary via displayed precomputed quantiles.
const CHARTABLE = new Set(['gauge', 'sum', 'histogram', 'exp_histogram', 'summary'])
export function isChartable(type: string): boolean {
  return CHARTABLE.has(type)
}

// Summary quantiles are precomputed and NOT re-aggregatable across series, so grouping is invalid.
export function groupByDisabled(type: string): boolean {
  return type === 'summary'
}
