// The attention strip caps at `max` — these tests exist because capping SILENTLY is the failure
// mode: eight cards on a fleet with thirty problems reads as "there are eight problems".
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import AttentionStrip from './AttentionStrip.vue'
import type { FleetRow } from '@/lib/services/servicesOverview'

const stubs = { AttentionCard: { props: ['row'], template: '<div class="card" :data-service="row.service" />' } }

function row(service: string, level: FleetRow['level']): FleetRow {
  return {
    service,
    environment: null,
    id: service,
    level,
    reasons: [],
    spread: { unhealthy: 0, total: 1, stale: 0 },
    sites: [],
    count: 100,
    span_count: 100,
    error_count: 0,
    error_rate: 0,
    http_4xx: 0,
    http_5xx: 0,
    p50: 0,
    p90: 0,
    p99: 0,
    apdex: 1,
    satisfied: 0,
    tolerating: 0,
    frustrated: 0,
    spark: [],
    error_spark: [],
    spanBased: false,
    incidents: [],
  }
}

describe('AttentionStrip', () => {
  it('renders a card per non-OK service, worst first', () => {
    const rows = [row('ok', 'ok'), row('warn', 'warning'), row('crit', 'critical'), row('down', 'down')]
    const w = mount(AttentionStrip, { props: { rows }, global: { stubs } })
    expect(w.findAll('.card').map((c) => c.attributes('data-service'))).toEqual(['down', 'crit', 'warn'])
  })

  it('renders nothing when everything is OK', () => {
    const w = mount(AttentionStrip, { props: { rows: [row('a', 'ok'), row('b', 'ok')] }, global: { stubs } })
    expect(w.find('[data-testid="attention-strip"]').exists()).toBe(false)
  })

  it('treats Unknown as not-attention — it is the absence of a judgment, not a grade', () => {
    const w = mount(AttentionStrip, { props: { rows: [row('a', 'unknown')] }, global: { stubs } })
    expect(w.find('[data-testid="attention-strip"]').exists()).toBe(false)
  })

  it('names what the cap hides instead of just dropping it', () => {
    const rows = Array.from({ length: 11 }, (_, i) => row('svc-' + i, 'critical'))
    const w = mount(AttentionStrip, { props: { rows, max: 8 }, global: { stubs } })
    expect(w.findAll('.card')).toHaveLength(8)
    expect(w.find('[data-testid="attention-overflow"]').text()).toContain('+3')
  })

  it('has no overflow chip when nothing is hidden', () => {
    const w = mount(AttentionStrip, { props: { rows: [row('a', 'critical')], max: 8 }, global: { stubs } })
    expect(w.find('[data-testid="attention-overflow"]').exists()).toBe(false)
  })
})
