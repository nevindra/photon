import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import MetricTiles from './MetricTiles.vue'

const tiles = [
  { label: 'Request rate', value: '120/s', delta: 0.12, tone: 'neutral' },
  { label: 'Error rate', value: '2.10%', delta: 0.3, tone: 'up-bad' },
  { label: 'p99', value: '9.1ms', delta: -0.05, tone: 'up-bad' },
  { label: 'Requests', value: '7,200', delta: null, tone: 'neutral' },
]

describe('MetricTiles', () => {
  it('renders a tile per entry with label + value', () => {
    const w = mount(MetricTiles, { props: { tiles } })
    expect(w.findAll('[data-testid="metric-tile"]').length).toBe(4)
    expect(w.text()).toContain('Request rate')
    expect(w.text()).toContain('120/s')
  })

  it('colors a rising up-bad delta red and a falling one green', () => {
    const w = mount(MetricTiles, { props: { tiles } })
    const deltas = w.findAll('[data-testid="metric-delta"]')
    // tiles[1]: error rate up (bad) → red
    expect(deltas[1].classes()).toContain('text-sev-error')
    // tiles[2]: p99 down (good) → green
    expect(deltas[2].classes().some((c) => c.startsWith('text-green'))).toBe(true)
  })

  it('omits the delta element when delta is null', () => {
    const w = mount(MetricTiles, { props: { tiles } })
    // Only 3 of 4 tiles have a non-null delta.
    expect(w.findAll('[data-testid="metric-delta"]').length).toBe(3)
  })

  it('renders a flat, muted indicator for a zero delta even under tone up-bad', () => {
    const flatTiles = [{ label: 'Error rate', value: '2.10%', delta: 0, tone: 'up-bad' }]
    const w = mount(MetricTiles, { props: { tiles: flatTiles } })
    const deltas = w.findAll('[data-testid="metric-delta"]')
    // delta 0 is not null, so it still renders — unlike a null delta.
    expect(deltas.length).toBe(1)
    expect(deltas[0].classes()).toContain('text-muted-foreground')
    expect(deltas[0].classes()).not.toContain('text-sev-error')
    expect(deltas[0].classes().some((c) => c.startsWith('text-green'))).toBe(false)
  })

  it('renders the comparison label under each tile when provided', () => {
    const w = mount(MetricTiles, { props: { tiles, comparisonLabel: 'vs prev 30m' } })
    const cells = w.findAll('[data-testid="metric-tile"]')
    expect(cells.length).toBe(4)
    expect(cells[0].text()).toContain('vs prev 30m')
  })

  it('colors an up-good delta green when rising and red when falling', () => {
    const t = [
      { label: 'Apdex up', value: '0.95', delta: 0.1, tone: 'up-good' },
      { label: 'Apdex down', value: '0.71', delta: -0.2, tone: 'up-good' },
    ]
    const w = mount(MetricTiles, { props: { tiles: t } })
    const d = w.findAll('[data-testid="metric-delta"]')
    expect(d[0].classes().some((c) => c.startsWith('text-green'))).toBe(true)
    expect(d[1].classes()).toContain('text-sev-error')
  })
})
