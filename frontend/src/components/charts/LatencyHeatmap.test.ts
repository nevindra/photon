// The heatmap. Two behaviours are load-bearing:
//
//   - The colour scale is per COLUMN. Scaled globally, one busy minute leaves every other column
//     near-black and the shape shifts the chart exists to show become invisible.
//   - Open spans (no duration) are reported apart from every band. "Under 1ms" and "never finished"
//     are opposite facts, and folding the second into the first makes a stalling operation look
//     like it got faster.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { TooltipProvider } from '@/components/ui/tooltip'
import LatencyHeatmap from './LatencyHeatmap.vue'
import type { LatencyHeatmap as Data } from '@/lib/core/api'

const BOUNDS = ['1000000', '2000000', '5000000', '10000000']

function mountChart(data: Data | null, thresholdMs: number | null = null) {
  return mount(
    { components: { TooltipProvider, LatencyHeatmap }, props: ['data', 'thresholdMs'],
      template: '<TooltipProvider><LatencyHeatmap :data="data" :threshold-ms="thresholdMs" /></TooltipProvider>' },
    { props: { data, thresholdMs } },
  )
}

const data = (cells: [number, number, number][], open = 0): Data => ({
  columns: [1000, 2000, 3000],
  bounds: BOUNDS,
  cells,
  open_spans: open,
})

const opacityOf = (w: ReturnType<typeof mount>, band: number, t: number) =>
  Number(w.find(`[data-cell="${band}:${t}"]`).attributes('style')?.match(/opacity:\s*([\d.]+)/)?.[1] ?? 0)

describe('LatencyHeatmap', () => {
  it('renders one row per band plus the overflow band', () => {
    const w = mountChart(data([[0, 0, 1]]))
    expect(w.findAll('[data-band]')).toHaveLength(BOUNDS.length + 1)
  })

  it('scales each column against its own busiest cell, not the whole chart', () => {
    // Column 0 has 10 spans, column 1 has 1000. Scaled globally, column 0 would be invisible.
    const w = mountChart(data([[0, 0, 10], [1, 0, 1000]]))
    expect(opacityOf(w, 0, 0)).toBe(1)
    expect(opacityOf(w, 0, 1)).toBe(1)
  })

  it('leaves an empty cell faint rather than invisible, so the grid keeps its shape', () => {
    const w = mountChart(data([[0, 0, 10]]))
    expect(opacityOf(w, 0, 2)).toBeGreaterThan(0)
    expect(opacityOf(w, 0, 2)).toBeLessThan(0.2)
  })

  it('reports open spans separately instead of folding them into the fastest band', () => {
    const w = mountChart(data([[0, 0, 5]], 7))
    expect(w.find('[data-testid="open-spans"]').text()).toContain('7')
  })

  it('says nothing about open spans when every span closed', () => {
    const w = mountChart(data([[0, 0, 5]]))
    expect(w.find('[data-testid="open-spans"]').exists()).toBe(false)
  })

  it('draws the Apdex T and 4T guides on the same axes as the distribution', () => {
    // T = 2ms lands in the <=2ms band (index 1); 4T = 8ms lands in the <=10ms band (index 3).
    const w = mountChart(data([[0, 0, 1]]), 2)
    expect(w.text()).toContain('4T')
    expect(w.find('[data-band="1"]').html()).toContain('T')
    expect(w.find('[data-band="3"]').html()).toContain('4T')
  })

  it('draws no guides without a threshold', () => {
    const w = mountChart(data([[0, 0, 1]]))
    expect(w.text()).not.toContain('4T')
  })

  it('says so when nothing matched', () => {
    expect(mountChart(data([])).text()).toContain('No spans in range')
  })
})
