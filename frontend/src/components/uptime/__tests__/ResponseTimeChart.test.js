import { describe, it, expect, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import ResponseTimeChart from '@/components/uptime/ResponseTimeChart.vue'
import LineChart from '@/components/charts/LineChart.vue'

afterEach(() => {
  document.body.innerHTML = ''
})

// Series with known ok latencies (10, 20, 40) plus a contiguous down run (idx 2–3, ts 3–4).
const SERIES = [
  { ts: 1, ok: true, latency_ms: 10 },
  { ts: 2, ok: true, latency_ms: 20 },
  { ts: 3, ok: false, latency_ms: 0 },
  { ts: 4, ok: false, latency_ms: 0 },
  { ts: 5, ok: true, latency_ms: 40 },
]

function mountChart(heartbeats) {
  return mount(ResponseTimeChart, {
    props: { heartbeats },
    global: { stubs: { LineChart: true } },
  })
}

describe('ResponseTimeChart', () => {
  it('maps heartbeats to a single latency series, down beats → v:null', () => {
    const w = mountChart(SERIES)
    const line = w.findComponent(LineChart)
    expect(line.props('series')).toEqual([
      {
        key: 'latency',
        label: 'Latency',
        color: expect.any(String),
        points: [
          { t: 1, v: 10 },
          { t: 2, v: 20 },
          { t: 3, v: null },
          { t: 4, v: null },
          { t: 5, v: 40 },
        ],
      },
    ])
  })

  it('forwards area:true and the ms window (first/last heartbeat ts)', () => {
    const w = mountChart(SERIES)
    const line = w.findComponent(LineChart)
    expect(line.props('area')).toBe(true)
    expect(line.props('startMs')).toBe(1)
    expect(line.props('endMs')).toBe(5)
  })

  it('computes an outage band spanning the down run, edges extended to neighboring beats', () => {
    const w = mountChart(SERIES)
    const bands = w.findComponent(LineChart).props('bands')
    expect(bands).toHaveLength(1)
    // down run is [ts:3, ts:4]; edges extend halfway to the adjacent ok beats (ts:2, ts:5).
    expect(bands[0]).toMatchObject({ x0Ms: 2.5, x1Ms: 4.5, label: 'outage' })
    expect(bands[0].color).toEqual(expect.any(String))
  })

  it('computes avg + p95 reference lines over the up-beat latencies', () => {
    const w = mountChart(SERIES)
    const refLines = w.findComponent(LineChart).props('refLines')
    // avg over [10, 20, 40] = 23.33 -> rounds to 23; p95 index floor(3*0.95)=2 -> sorted[2] = 40.
    expect(refLines).toEqual([
      { y: 23, label: 'avg', color: expect.any(String) },
      { y: 40, label: 'p95', color: expect.any(String) },
    ])
  })

  it('shows the min/avg/p95/max stat header', () => {
    const w = mountChart(SERIES)
    const text = w.text()
    expect(text).toContain('min')
    expect(text).toContain('avg')
    expect(text).toContain('p95')
    expect(text).toContain('max')
    // min = 10, max = 40 over the ok latencies
    expect(text).toContain('10')
    expect(text).toContain('40')
  })

  it('shows an empty state and no chart when there are no ok points', () => {
    const empty = mountChart([])
    expect(empty.text()).toContain('no data yet')
    expect(empty.findComponent(LineChart).exists()).toBe(false)

    const allDown = mountChart([
      { ts: 1, ok: false, latency_ms: 0 },
      { ts: 2, ok: false, latency_ms: 0 },
    ])
    expect(allDown.text()).toContain('no data yet')
    expect(allDown.findComponent(LineChart).exists()).toBe(false)
  })
})
