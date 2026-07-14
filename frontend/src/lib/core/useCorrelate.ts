// One source of truth for cross-signal links. correlate() ALWAYS merges the active time window +
// scope from context.ts, so no navigation hop can drop context. relatedFor() returns the typed
// destination list per entity kind, dropping any destination whose required field is absent.
import { timeRange, customRange, scope } from '@/lib/core/context'

export interface Destination {
  path: string
  query?: Record<string, unknown>
}

export function correlate(dest: Destination): string {
  const { path, query = {} } = dest
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(query)) {
    if (v != null && v !== '') p.set(k, String(v))
  }
  if (customRange.value) {
    p.set('from', String(customRange.value.startMs))
    p.set('to', String(customRange.value.endMs))
  } else if (timeRange.value) {
    p.set('range', timeRange.value)
  }
  if (scope.value) p.set('scope', `${scope.value.type}:${scope.value.id}`)
  const qs = p.toString()
  return path + (qs ? `?${qs}` : '')
}

export type EntityKind = 'log' | 'span' | 'service' | 'rumError' | 'rumPage' | 'monitor' | 'metric' | 'host'

export interface Entity {
  kind: EntityKind | string
  fields?: Record<string, string | undefined>
}

export interface RelatedDestination {
  id: string
  label: string
  dest: Destination
  phase?: 2
}

// A span/name term is only expressible if it has no whitespace/comma (grammar splits on those) —
// mirrors ServiceDetailView.onOpenExemplars' guard.
const term = (field: string, val: string | undefined): string =>
  val && !/[\s,]/.test(val) ? `${field}:${val}` : ''
const andTerms = (...ts: string[]): string => ts.filter(Boolean).join(' ')

type Fields = Record<string, string | undefined>
type Candidate = RelatedDestination | false | undefined

// Each builder returns candidate destinations; those referencing a missing field are filtered out.
function candidates(kind: string, f: Fields): Candidate[] {
  switch (kind) {
    case 'log':
      return [
        Boolean(f.traceId) && { id: 'trace', label: 'Trace waterfall', dest: { path: `/traces/${f.traceId}` } },
        Boolean(f.service) && { id: 'service-health', label: `${f.service} · Backend health`, dest: { path: `/services/${encodeURIComponent(f.service ?? '')}` } },
      ]
    case 'span':
      return [
        Boolean(f.traceId && f.spanId) && { id: 'logs-span', label: 'Logs for this span', dest: { path: '/logs', query: { q: andTerms(term('trace_id', f.traceId), term('span_id', f.spanId)) } } },
        Boolean(f.traceId) && { id: 'logs-trace', label: 'Logs for this trace', dest: { path: '/logs', query: { q: term('trace_id', f.traceId) } } },
        Boolean(f.service) && { id: 'service-health', label: `${f.service} · Backend health`, dest: { path: `/services/${encodeURIComponent(f.service ?? '')}` } },
        Boolean(f.service) && { id: 'similar-traces', label: 'Similar traces', dest: { path: '/traces', query: { q: andTerms(term('service.name', f.service), term('name', f.operation)), sort: 'slowest' } } },
      ]
    case 'service':
      return [
        { id: 'traces', label: 'Traces', dest: { path: '/traces', query: { q: term('service.name', f.service), sort: 'slowest' } } },
        { id: 'logs', label: 'Logs', dest: { path: '/logs', query: { q: term('service.name', f.service) } } },
        { id: 'metrics', label: 'Metrics', dest: { path: '/metrics', query: { svc: f.service } } },
        { id: 'rum-app', label: 'RUM app', dest: { path: '/rum', query: { svc: f.service } } },
        { id: 'uptime', label: 'Uptime', dest: { path: '/uptime', query: { svc: f.service } } },
      ]
    case 'rumError':
      return [
        Boolean(f.traceId) && { id: 'trace', label: 'Trace waterfall', dest: { path: `/traces/${f.traceId}` } },
        { id: 'logs', label: 'Logs', dest: { path: '/logs', query: { q: f.traceId ? term('trace_id', f.traceId) : term('service.name', f.service) } } },
        Boolean(f.service) && { id: 'service', label: 'Backend service', dest: { path: `/services/${encodeURIComponent(f.service ?? '')}` } },
      ]
    case 'rumPage':
      return [
        { id: 'traces', label: 'Traces', dest: { path: '/traces', query: { q: term('service.name', f.service), sort: 'slowest' } } },
      ]
    case 'monitor':
      return [
        Boolean(f.service) && { id: 'service', label: 'Service', dest: { path: `/services/${encodeURIComponent(f.service ?? '')}` } },
        Boolean(f.service) && { id: 'logs', label: 'Logs', dest: { path: '/logs', query: { q: term('service.name', f.service) } } },
        Boolean(f.service) && { id: 'traces', label: 'Traces', dest: { path: '/traces', query: { q: term('service.name', f.service), sort: 'slowest' } } },
      ]
    case 'metric':
      return [
        { id: 'exemplar-traces', label: 'Exemplar traces', dest: { path: '/traces', query: { q: term('service.name', f.service), sort: 'slowest' } } },
        Boolean(f.service) && { id: 'logs', label: 'Logs', dest: { path: '/logs', query: { q: term('service.name', f.service) } } },
      ]
    case 'host':
      return [
        { id: 'logs', label: 'Logs', dest: { path: '/logs', query: { q: term('host.name', f.host) } } },
        { id: 'traces', label: 'Traces', dest: { path: '/traces', query: { q: term('host.name', f.host), sort: 'slowest' } } },
        { id: 'metrics', label: 'Metrics', dest: { path: '/metrics' } },
      ]
    default:
      return []
  }
}

export function relatedFor(entity: Entity): RelatedDestination[] {
  const list = candidates(entity.kind, entity.fields ?? {}).filter((c): c is RelatedDestination => Boolean(c))
  // Phase-2 flagship destination, present on the correlated kinds.
  if (['span', 'log', 'service', 'rumError'].includes(entity.kind)) {
    list.push({ id: 'correlated-view', label: 'Open correlated view', dest: { path: '/traces' }, phase: 2 })
  }
  return list
}
