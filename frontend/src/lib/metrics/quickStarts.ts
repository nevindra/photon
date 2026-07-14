// Quick-start builder-fills. Two kinds: curated (keyed on standard metric names, surfaced only
// when present in the catalog) and type-relative presets (agg chips for the selected metric type).
import type { MetricEntry } from '@/lib/metrics/metricNamespaces'

export interface QuickStart {
  metric: string
  agg: string
  group_by?: string[]
  viz?: string
  label: string
  description: string
}

interface CuratedDef {
  match: string[] // accepted spellings (OTEL dot form + Prometheus underscore form)
  agg: string
  viz?: string
  label: string
  description: string
}

const CURATED: CuratedDef[] = [
  {
    match: [
      'http.server.request.duration', 'http.server.duration',
      'http_server_request_duration_seconds', 'http_request_duration_seconds',
    ],
    agg: 'p99', viz: 'line',
    label: 'HTTP p99 latency', description: '99th-percentile server request duration',
  },
  {
    match: [
      'http.server.request.count', 'http.server.requests',
      'http_requests_total', 'http_server_requests_total',
    ],
    agg: 'rate', viz: 'line',
    label: 'Request throughput', description: 'Requests per second',
  },
  {
    match: ['http.server.active_requests', 'http_server_active_requests'],
    agg: 'avg', viz: 'line',
    label: 'In-flight requests', description: 'Concurrent active requests',
  },
]

export function curatedQuickStarts(catalog: MetricEntry[]): QuickStart[] {
  const names = new Set(catalog.map((e) => e.name))
  const out: QuickStart[] = []
  for (const def of CURATED) {
    const metric = def.match.find((m) => names.has(m))
    if (!metric) continue
    out.push({ metric, agg: def.agg, viz: def.viz, label: def.label, description: def.description })
  }
  return out
}

// Aggregation presets per metric type. Mirrors aggOptionsForType (metricFields.js) — keep in sync.
export function presetsForType(type: string, isMonotonic: boolean | null | undefined): string[] {
  switch (type) {
    case 'gauge':
      return ['avg', 'max', 'min', 'last']
    case 'sum':
      return isMonotonic ? ['rate', 'increase', 'sum'] : ['sum', 'rate']
    case 'histogram':
    case 'exp_histogram':
      return ['p99', 'p90', 'p50', 'count']
    case 'summary':
      return ['median', 'p90', 'p99']
    default:
      return []
  }
}
