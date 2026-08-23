// The TanStack Charts adapter. jsdom does not lay out SVG, so what is pinned here is the mapping
// and the guard rails — the long-shape flatten, the empty state, and the fixed x domain — not
// pixels. The engine's own rendering is its business.
import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import StackedBandChart from './StackedBandChart.vue'

// The engine is stubbed so the assertions are about what this component HANDS it. Mounting the
// real one in jsdom would test @tanstack/charts, which is not our code.
const { Chart } = vi.hoisted(() => ({
  Chart: {
    props: ['definition', 'ariaLabel', 'height'],
    template: '<svg data-chart />',
  },
}))
vi.mock('@tanstack/charts/vue', () => ({ Chart }))

function bucket(t: number, satisfied: number, frustrated: number) {
  return {
    t,
    segments: [
      { key: 'satisfied', label: 'Satisfied', color: 'hsl(160 84% 39%)', value: satisfied },
      { key: 'frustrated', label: 'Frustrated', color: 'hsl(0 72% 51%)', value: frustrated },
    ],
  }
}

const T0 = 1_700_000_000_000

function mountChart(props: Record<string, unknown> = {}) {
  return mount(StackedBandChart, {
    props: { startMs: T0, endMs: T0 + 3_600_000, ...props } as never,
  })
}

describe('StackedBandChart', () => {
  it('says the window is empty rather than drawing axes around nothing', () => {
    const w = mountChart({ buckets: [] })
    expect(w.find('[data-testid="tanstack-stacked-empty"]').text()).toBe('No data in range')
    expect(w.find('[data-testid="tanstack-stacked-chart"]').exists()).toBe(false)
  })

  it('distinguishes loading from empty', () => {
    const w = mountChart({ buckets: [], loading: true })
    expect(w.find('[data-testid="tanstack-stacked-empty"]').text()).toBe('Loading…')
  })

  it('treats all-zero buckets as empty — a row of zero-height bars is not a chart', () => {
    const w = mountChart({ buckets: [bucket(T0, 0, 0), bucket(T0 + 60_000, 0, 0)] })
    expect(w.find('[data-testid="tanstack-stacked-empty"]').exists()).toBe(true)
  })

  it('renders once any bucket has a value', () => {
    const w = mountChart({ buckets: [bucket(T0, 5, 1)] })
    expect(w.find('[data-testid="tanstack-stacked-chart"]').exists()).toBe(true)
  })

  it('pins the x domain to the caller"s window, not to the data', () => {
    // A chart whose axis stops at the last bucket with data silently rescales when a service goes
    // quiet, which reads to the viewer as the time range having changed.
    const w = mountChart({ buckets: [bucket(T0, 5, 1)] })
    const def = w.findComponent(Chart).props('definition') as { x?: { scale: () => unknown } }
    const scale = def.x?.scale() as { domain: () => number[] }
    expect(scale.domain()).toEqual([T0, T0 + 3_600_000])
  })

  it('takes the accessible name from the caller — the engine requires one', () => {
    const w = mountChart({ buckets: [bucket(T0, 5, 1)], ariaLabel: 'Apdex bands over time' })
    expect(w.findComponent(Chart).props('ariaLabel')).toBe('Apdex bands over time')
  })

  it('forwards the height so the panel it sits in does not jump', () => {
    const w = mountChart({ buckets: [bucket(T0, 5, 1)], height: 180 })
    expect(w.findComponent(Chart).props('height')).toBe(180)
    expect(w.find('div').attributes('style')).toContain('height: 180px')
  })

  it('builds a fresh definition when the data changes, not a mutated one', async () => {
    // The engine re-reads a definition by IDENTITY. A mutated-in-place spec would keep painting the
    // previous data — and, worse, the previous theme after a light↔dark flip.
    const w = mountChart({ buckets: [bucket(T0, 5, 1)] })
    const first = w.findComponent(Chart).props('definition')
    await w.setProps({ buckets: [bucket(T0, 9, 2)] } as never)
    expect(w.findComponent(Chart).props('definition')).not.toBe(first)
  })
})
