import { describe, it, expect } from 'vitest'
import {
  ALERT_TEMPLATES,
  templatesForTarget,
  templateToRuleInput,
  summarizeCondition,
} from './alertTemplates'

describe('alert template catalog', () => {
  it('has 23 templates with unique ids', () => {
    expect(ALERT_TEMPLATES).toHaveLength(23)
    expect(new Set(ALERT_TEMPLATES.map((t) => t.id)).size).toBe(23)
  })

  it('counts per target type', () => {
    expect(templatesForTarget('service')).toHaveLength(7)
    expect(templatesForTarget('app')).toHaveLength(6)
    expect(templatesForTarget('host')).toHaveLength(6)
    expect(templatesForTarget('global')).toHaveLength(4)
  })

  it('never uses the engine-rejected p95 aggregations/kinds', () => {
    for (const t of ALERT_TEMPLATES) {
      const c = t.build('x')
      if (c.signal === 'metrics') expect(c.agg).not.toBe('p95')
      if (c.signal === 'traces') expect(c.kind).not.toBe('latency_p95')
    }
  })

  it('substitutes the target into the right field per target type', () => {
    const svc = templatesForTarget('service').map((t) => t.build('checkout-api'))
    for (const c of svc) {
      if (c.signal === 'traces') expect(c.service).toBe('checkout-api')
      if (c.signal === 'logs') expect(c.query).toContain('service.name:checkout-api')
    }
    for (const t of templatesForTarget('app')) {
      const c = t.build('storefront')
      expect(c.signal).toBe('rum')
      if (c.signal === 'rum') expect(c.app_id).toBe('storefront')
    }
    for (const t of templatesForTarget('host')) {
      const c = t.build('web-01')
      expect(c.signal).toBe('metrics')
      if (c.signal === 'metrics') expect(c.label_filters?.['host.name']).toBe('web-01')
    }
    for (const t of templatesForTarget('global')) {
      const c = t.build('')
      expect(c.signal).toBe('metrics')
      if (c.signal === 'metrics') expect(c.group_by).toContain('host.name')
    }
  })

  it('utilization host/fleet templates threshold within (0, 1]', () => {
    for (const t of [...templatesForTarget('host'), ...templatesForTarget('global')]) {
      const c = t.build('h')
      if (c.signal === 'metrics' && c.metric_name.endsWith('.utilization')) {
        expect(c.threshold).toBeGreaterThan(0)
        expect(c.threshold).toBeLessThanOrEqual(1)
      }
    }
  })

  it('templateToRuleInput composes name and defaults', () => {
    const t = templatesForTarget('service')[0]
    const input = templateToRuleInput(t, 'checkout-api', ['ch1'])
    expect(input.name).toBe(`${t.name} · checkout-api`)
    expect(input.enabled).toBe(true)
    expect(input.interval_secs).toBe(60)
    expect(input.channel_ids).toEqual(['ch1'])
    expect(input.severity).toBe(t.severity)

    const g = templatesForTarget('global')[0]
    expect(templateToRuleInput(g, '', []).name).toBe(g.name) // no ` · ` suffix for global
  })

  it('summarizeCondition renders a readable line', () => {
    const c = templatesForTarget('service')[0].build('checkout-api')
    expect(summarizeCondition(c)).toMatch(/error_rate/)
  })
})
