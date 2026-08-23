// The one-hop map. Two things are load-bearing: it never quietly drops rows (a picture you cannot
// trust is worse than a table), and only Services are clickable — a database has no page.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import FocusedDependencyMap, { type MapSideNode } from './FocusedDependencyMap.vue'
import type { ServiceMapEdge } from '@/lib/core/api'

function edge(from: string, to: string, count = 100, error_rate = 0): ServiceMapEdge {
  return {
    from,
    to,
    count,
    rate: count / 60,
    error_count: Math.round(count * error_rate),
    error_rate,
    p50: '1000000',
    p95: '4000000',
    p99: '9000000',
  }
}

function dep(name: string, kind: 'database' | 'external', count = 50): MapSideNode {
  return { key: `${kind}:${name}`, label: name, kind, count, errorRate: 0, p99: '2000000' }
}

function mountMap(props: Partial<InstanceType<typeof FocusedDependencyMap>['$props']> = {}) {
  return mount(FocusedDependencyMap, {
    props: { service: 'api', callers: [], callees: [], ...props } as never,
  })
}

describe('FocusedDependencyMap', () => {
  it('draws callers on the left and callees on the right', () => {
    const w = mountMap({
      callers: [edge('web', 'api')],
      callees: [edge('api', 'auth-svc')],
    })
    expect(w.find('[data-map-node="web"][data-side="caller"]').exists()).toBe(true)
    expect(w.find('[data-map-node="auth-svc"][data-side="callee"]').exists()).toBe(true)
  })

  it('puts databases and external calls in the same picture as service callees', () => {
    const w = mountMap({
      callees: [edge('api', 'auth-svc')],
      dependencies: [dep('postgres', 'database'), dep('stripe.com', 'external')],
    })
    const right = w.findAll('[data-side="callee"]').map((n) => n.attributes('data-map-node'))
    expect(right).toEqual(expect.arrayContaining(['auth-svc', 'postgres', 'stripe.com']))
  })

  it('orders each column by call volume, busiest first', () => {
    const w = mountMap({
      callees: [edge('quiet', 'x', 10), edge('busy', 'y', 900)],
      // (both are `from: api` in reality; what is pinned is the ordering by count)
    })
    const right = w.findAll('[data-side="callee"]').map((n) => n.attributes('data-map-node'))
    expect(right[0]).toBe('y')
  })

  it('says how many rows it did not draw rather than dropping them silently', () => {
    const callees = Array.from({ length: 12 }, (_, i) => edge('api', `svc-${i}`, 100 - i))
    const w = mountMap({ callees, max: 5 })
    const note = w.find('[data-testid="focused-map-overflow"]')
    expect(note.exists()).toBe(true)
    expect(note.text()).toContain('7 more downstream')
  })

  it('draws no overflow note when everything fits', () => {
    const w = mountMap({ callees: [edge('api', 'auth-svc')], max: 5 })
    expect(w.find('[data-testid="focused-map-overflow"]').exists()).toBe(false)
  })

  it('emits select for a service box but not for a database', async () => {
    const w = mountMap({
      callees: [edge('api', 'auth-svc')],
      dependencies: [dep('postgres', 'database')],
    })
    await w.find('[data-map-node="auth-svc"]').trigger('click')
    await w.find('[data-map-node="postgres"]').trigger('click')
    const emitted = w.emitted('select') ?? []
    expect(emitted).toHaveLength(1)
    expect((emitted[0][0] as MapSideNode).label).toBe('auth-svc')
  })

  it('renders the service itself even with nothing on either side', () => {
    const w = mountMap()
    expect(w.find('[data-testid="focused-map"]').exists()).toBe(true)
    expect(w.text()).toContain('api')
    expect(w.text()).toContain('0 in · 0 out')
  })
})
