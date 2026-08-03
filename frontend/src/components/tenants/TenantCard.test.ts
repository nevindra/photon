// One tenant in the Home tenant board: name + mode badge + status dot, ingest/incidents/hot-bytes
// rows, a sparkline, and a relative last-seen footer. Real rendered behavior, mirrors HostCard.test.ts.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import TenantCard from './TenantCard.vue'
import type { TenantSummary } from '@/lib/core/api'

function makeTenant(overrides: Partial<TenantSummary> = {}): TenantSummary {
  return {
    name: 'acme',
    mode: 'summary',
    status: 'up',
    last_seen_ms: Date.now() - 12_000,
    ingest_rows_per_sec: 42,
    open_incidents: 0,
    hot_bytes: 210_000_000,
    ui_url: 'https://acme.example.com',
    spark: [[0, 1], [1, 2], [2, 3]],
    ...overrides,
  }
}

describe('TenantCard', () => {
  it('renders the tenant name, mode badge and stat rows from a fixture', () => {
    const w = mount(TenantCard, { props: { tenant: makeTenant() } })
    expect(w.text()).toContain('acme')
    expect(w.text()).toContain('summary')
    expect(w.text()).toContain('42')
    expect(w.text()).toMatch(/ago/)
  })

  it('emits select with the tenant on click', async () => {
    const tenant = makeTenant()
    const w = mount(TenantCard, { props: { tenant } })
    await w.trigger('click')
    expect(w.emitted('select')?.[0]).toEqual([tenant])
  })

  it('shows an unreachable treatment when status is not up', () => {
    const w = mount(TenantCard, { props: { tenant: makeTenant({ status: 'down' }) } })
    expect(w.text()).toContain('Unreachable')
    expect(w.html()).toMatch(/sev-error/)

    const healthy = mount(TenantCard, { props: { tenant: makeTenant() } })
    expect(healthy.text()).not.toContain('Unreachable')
  })

  it('shows an Open UI link for summary-mode tenants with a ui_url, not for full-mode', () => {
    const summary = mount(TenantCard, { props: { tenant: makeTenant() } })
    expect(summary.find('a[href="https://acme.example.com"]').exists()).toBe(true)

    const full = mount(TenantCard, { props: { tenant: makeTenant({ mode: 'full' }) } })
    expect(full.find('a').exists()).toBe(false)
  })
})
