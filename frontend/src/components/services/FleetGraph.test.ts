// The fleet graph. The layout library loads asynchronously, so every assertion here waits for it —
// which also proves the dynamic import actually resolves under the test runner rather than being a
// code path nobody has ever executed.
import { describe, it, expect } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import FleetGraph from './FleetGraph.vue'
import type { ServiceMapEdge, ServiceMapNode } from '@/lib/core/api'

function node(service: string, count = 1000, error_rate = 0): ServiceMapNode {
  return {
    service,
    count,
    rate: count / 60,
    error_count: Math.round(count * error_rate),
    error_rate,
    roots: 0,
    is_entry: false,
  }
}

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

async function mountGraph(props: Record<string, unknown>) {
  const w = mount(FleetGraph, { props: { nodes: [], edges: [], ...props } as never })
  await flushPromises()
  await flushPromises()
  return w
}

describe('FleetGraph', () => {
  it('lays out a node per service and a path per edge', async () => {
    const w = await mountGraph({
      nodes: [node('web'), node('api'), node('postgres')],
      edges: [edge('web', 'api'), edge('api', 'postgres')],
    })
    expect(w.findAll('[data-graph-node]')).toHaveLength(3)
    expect(w.find('[data-edge="web->api"]').exists()).toBe(true)
    expect(w.find('[data-edge="api->postgres"]').exists()).toBe(true)
  })

  it('gives the svg a real viewBox once laid out', async () => {
    const w = await mountGraph({ nodes: [node('web'), node('api')], edges: [edge('web', 'api')] })
    const box = w.find('[data-testid="fleet-graph"]').attributes('viewBox') ?? ''
    const [, , width, height] = box.split(' ').map(Number)
    expect(width).toBeGreaterThan(0)
    expect(height).toBeGreaterThan(0)
  })

  it('drops an edge naming a service with no node rather than inventing one', async () => {
    const w = await mountGraph({
      nodes: [node('web')],
      edges: [edge('web', 'ghost')],
    })
    expect(w.findAll('[data-graph-node]')).toHaveLength(1)
    expect(w.find('[data-edge="web->ghost"]').exists()).toBe(false)
  })

  it('weights an edge by throughput and colours it by error rate — separate channels', async () => {
    const w = await mountGraph({
      nodes: [node('a'), node('b'), node('c'), node('d')],
      edges: [edge('a', 'b', 1000, 0), edge('c', 'd', 10, 0.5)],
    })
    const busy = w.find('[data-edge="a->b"]')
    const broken = w.find('[data-edge="c->d"]')
    // Busy-and-healthy is thick and neutral; quiet-and-broken is thin and red.
    expect(Number(busy.attributes('stroke-width'))).toBeGreaterThan(
      Number(broken.attributes('stroke-width')),
    )
    expect(busy.attributes('stroke')).toContain('--border')
    expect(broken.attributes('stroke')).toContain('--sev-error')
  })

  it('colours a node dot from the shared Attention Level, not from its own span counts', async () => {
    const w = await mountGraph({
      nodes: [node('api', 1_000_000, 0)], // busy and error-free…
      edges: [],
      levels: { api: 'down' }, // …but the fleet says it is Down (silence), and that wins.
    })
    expect(w.find('[data-graph-node="api"] circle').attributes('fill')).toContain('--sev-error')
  })

  it('treats a service with no level as Unknown rather than OK', async () => {
    const w = await mountGraph({ nodes: [node('api')], edges: [] })
    expect(w.find('[data-graph-node="api"] circle').attributes('fill')).toContain(
      '--muted-foreground',
    )
  })

  it('emits the service name on click', async () => {
    const w = await mountGraph({ nodes: [node('api')], edges: [] })
    await w.find('[data-graph-node="api"]').trigger('click')
    expect(w.emitted('select')?.[0]).toEqual(['api'])
  })

  it('dims everything not touching the focused service', async () => {
    const w = await mountGraph({
      nodes: [node('a'), node('b'), node('c'), node('d')],
      edges: [edge('a', 'b'), edge('c', 'd')],
      focus: 'a',
    })
    expect(Number(w.find('[data-edge="a->b"]').attributes('opacity'))).toBeGreaterThan(
      Number(w.find('[data-edge="c->d"]').attributes('opacity')),
    )
  })

  it('says nothing reported rather than drawing an empty canvas', async () => {
    const w = await mountGraph({ nodes: [], edges: [] })
    expect(w.find('[data-testid="fleet-graph"]').exists()).toBe(false)
    expect(w.text()).toContain('No services in range')
  })

  it('handles a cycle without hanging or dropping an edge', async () => {
    const w = await mountGraph({
      nodes: [node('a'), node('b')],
      edges: [edge('a', 'b'), edge('b', 'a')],
    })
    expect(w.find('[data-edge="a->b"]').exists()).toBe(true)
    expect(w.find('[data-edge="b->a"]').exists()).toBe(true)
  })
})
