// Static, read-only catalog of ready-made alert rules ("quick setup"). Frontend-only seed data:
// picking a template in TemplatePickerDialog either Applies it (POST /api/alerts/rules straight
// from templateToRuleInput) or Customizes it (opens AlertRuleDialog pre-seeded). `build(target)`
// substitutes the install-specific entity into the right field per target type. See
// docs/superpowers/specs/2026-07-18-alert-rule-templates-design.md.
import type { AlertCondition, AlertRuleInput, AlertSeverity } from '@/lib/core/api'

export type TemplateTarget = 'service' | 'app' | 'host' | 'global'

export interface AlertTemplate {
  id: string
  target: TemplateTarget
  name: string
  description: string
  severity: AlertSeverity
  for_secs: number
  interval_secs?: number
  /** Return a concrete condition with the target substituted in. */
  build: (target: string) => AlertCondition
}

// --- builders (keep DRY; each returns a fully-typed AlertCondition) ---------------------------
const traces = (
  kind: Extract<AlertCondition, { signal: 'traces' }>['kind'],
  window_secs: number,
  cmp: AlertCondition['cmp'],
  threshold: number,
) => (service: string): AlertCondition => ({ signal: 'traces', service, kind, window_secs, cmp, threshold })

const logs = (severity: string, window_secs: number, threshold: number) =>
  (service: string): AlertCondition => ({
    signal: 'logs',
    query: `service.name:${service} severity:${severity}`,
    group_by: null,
    window_secs,
    cmp: 'gt',
    threshold,
  })

const rum = (
  kind: Extract<AlertCondition, { signal: 'rum' }>['kind'],
  threshold: number,
) => (app_id: string): AlertCondition => ({
  signal: 'rum',
  app_id,
  route: null,
  kind,
  window_secs: 900,
  cmp: 'gt',
  threshold,
})

const hostMetric = (
  metric_name: string,
  agg: Extract<AlertCondition, { signal: 'metrics' }>['agg'],
  window_secs: number,
  threshold: number,
) => (host: string): AlertCondition => ({
  signal: 'metrics',
  metric_name,
  label_filters: { 'host.name': host },
  agg,
  window_secs,
  cmp: 'gt',
  threshold,
})

const fleetMetric = (
  metric_name: string,
  agg: Extract<AlertCondition, { signal: 'metrics' }>['agg'],
  window_secs: number,
  threshold: number,
) => (): AlertCondition => ({
  signal: 'metrics',
  metric_name,
  group_by: ['host.name'],
  agg,
  window_secs,
  cmp: 'gt',
  threshold,
})

export const ALERT_TEMPLATES: AlertTemplate[] = [
  // --- Service (traces + logs) ---
  { id: 'svc-high-error-rate', target: 'service', name: 'High error rate', description: 'Error rate above 5% over 5m.', severity: 'critical', for_secs: 300, build: traces('error_rate', 300, 'gt', 5) },
  { id: 'svc-elevated-error-rate', target: 'service', name: 'Elevated error rate', description: 'Error rate above 1% over 5m.', severity: 'warning', for_secs: 600, build: traces('error_rate', 300, 'gt', 1) },
  { id: 'svc-slow-p99', target: 'service', name: 'Slow responses (p99)', description: 'p99 latency above 1000ms over 5m.', severity: 'warning', for_secs: 300, build: traces('latency_p99', 300, 'gt', 1000) },
  { id: 'svc-slow-p90', target: 'service', name: 'Slow responses (p90)', description: 'p90 latency above 500ms over 5m.', severity: 'warning', for_secs: 600, build: traces('latency_p90', 300, 'gt', 500) },
  { id: 'svc-traffic-dropped', target: 'service', name: 'Traffic dropped', description: 'Request rate below 1 req/s over 10m — the service may be down.', severity: 'warning', for_secs: 600, build: traces('request_rate', 600, 'lt', 1) },
  { id: 'svc-error-logs', target: 'service', name: 'Error logs surging', description: 'More than 100 error logs over 10m.', severity: 'warning', for_secs: 300, build: logs('error', 600, 100) },
  { id: 'svc-fatal-logs', target: 'service', name: 'Fatal logs appeared', description: 'Any fatal log over 5m.', severity: 'critical', for_secs: 0, build: logs('fatal', 300, 0) },

  // --- RUM app ---
  { id: 'rum-poor-lcp', target: 'app', name: 'Poor LCP', description: 'LCP p75 above 2500ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_lcp_p75', 2500) },
  { id: 'rum-poor-inp', target: 'app', name: 'Poor INP', description: 'INP p75 above 200ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_inp_p75', 200) },
  { id: 'rum-poor-cls', target: 'app', name: 'Layout shift (CLS)', description: 'CLS p75 above 0.1 over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_cls_p75', 0.1) },
  { id: 'rum-slow-fcp', target: 'app', name: 'Slow FCP', description: 'FCP p75 above 1800ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_fcp_p75', 1800) },
  { id: 'rum-slow-ttfb', target: 'app', name: 'Slow TTFB', description: 'TTFB p75 above 800ms over 15m.', severity: 'warning', for_secs: 0, build: rum('vital_ttfb_p75', 800) },
  { id: 'rum-js-errors', target: 'app', name: 'JS errors surging', description: 'More than 50 JS errors over 15m.', severity: 'warning', for_secs: 0, build: rum('error_count', 50) },

  // --- Host (one host) ---
  { id: 'host-cpu', target: 'host', name: 'CPU saturated', description: 'CPU utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.cpu.utilization', 'avg', 300, 0.9) },
  { id: 'host-mem', target: 'host', name: 'Memory pressure', description: 'Memory utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.memory.utilization', 'avg', 300, 0.9) },
  { id: 'host-disk', target: 'host', name: 'Disk filling up', description: 'Filesystem utilization above 85% over 10m.', severity: 'warning', for_secs: 600, build: hostMetric('system.filesystem.utilization', 'avg', 600, 0.85) },
  { id: 'host-gpu', target: 'host', name: 'GPU saturated', description: 'GPU utilization above 95% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.gpu.utilization', 'avg', 300, 0.95) },
  { id: 'host-gpu-temp', target: 'host', name: 'GPU overheating', description: 'GPU temperature above 85°C over 5m.', severity: 'critical', for_secs: 300, build: hostMetric('system.gpu.temperature', 'max', 300, 85) },
  { id: 'host-gpu-mem', target: 'host', name: 'GPU memory pressure', description: 'GPU memory utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: hostMetric('system.gpu.memory.utilization', 'avg', 300, 0.9) },

  // --- Global / fleet (one series per host) ---
  { id: 'fleet-cpu', target: 'global', name: 'Any host CPU saturated', description: 'Any host CPU utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: fleetMetric('system.cpu.utilization', 'avg', 300, 0.9) },
  { id: 'fleet-mem', target: 'global', name: 'Any host memory pressure', description: 'Any host memory utilization above 90% over 5m.', severity: 'warning', for_secs: 300, build: fleetMetric('system.memory.utilization', 'avg', 300, 0.9) },
  { id: 'fleet-disk', target: 'global', name: 'Any host disk filling', description: 'Any host filesystem utilization above 85% over 10m.', severity: 'warning', for_secs: 600, build: fleetMetric('system.filesystem.utilization', 'avg', 600, 0.85) },
  { id: 'fleet-gpu-temp', target: 'global', name: 'Any GPU overheating', description: 'Any GPU temperature above 85°C over 5m.', severity: 'critical', for_secs: 300, build: fleetMetric('system.gpu.temperature', 'max', 300, 85) },
]

export function templatesForTarget(target: TemplateTarget): AlertTemplate[] {
  return ALERT_TEMPLATES.filter((t) => t.target === target)
}

export function templateToRuleInput(
  t: AlertTemplate,
  target: string,
  channelIds: string[],
): AlertRuleInput {
  const condition = t.build(target)
  const suffix = t.target === 'global' || !target ? '' : ` · ${target}`
  return {
    name: `${t.name}${suffix}`,
    description: t.description,
    enabled: true,
    signal: condition.signal,
    condition,
    for_secs: t.for_secs,
    interval_secs: t.interval_secs ?? 60,
    severity: t.severity,
    channel_ids: channelIds,
  }
}

const CMP_SYM: Record<AlertCondition['cmp'], string> = { gt: '>', gte: '≥', lt: '<', lte: '≤' }

export function fmtSecs(s: number): string {
  if (s === 0) return 'immediately'
  if (s % 3600 === 0) return `${s / 3600}h`
  if (s % 60 === 0) return `${s / 60}m`
  return `${s}s`
}

/** A compact plain-English line for a template row / preview (numbers only; target-agnostic). */
export function summarizeCondition(c: AlertCondition): string {
  const win = `over ${fmtSecs(c.window_secs)}`
  const op = CMP_SYM[c.cmp]
  switch (c.signal) {
    case 'metrics': {
      const by = c.group_by?.length ? ` by ${c.group_by.join(', ')}` : ''
      return `${c.agg}(${c.metric_name})${by} ${op} ${c.threshold} ${win}`
    }
    case 'logs':
      return `count(logs) ${op} ${c.threshold} ${win}`
    case 'traces':
      return `${c.kind} ${op} ${c.threshold} ${win}`
    case 'rum':
      return `${c.kind} ${op} ${c.threshold} ${win}`
  }
}
