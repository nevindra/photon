import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HeartbeatBar from '@/components/uptime/HeartbeatBar.vue'

describe('HeartbeatBar', () => {
  it('renders one tick per heartbeat (capped at max)', () => {
    const heartbeats = Array.from({ length: 50 }, (_, i) => ({ ok: i % 2 === 0, ts: i, latency_ms: 10 }))
    const w = mount(HeartbeatBar, { props: { heartbeats, max: 40 } })
    expect(w.findAll('[data-tick]').length).toBe(40)
  })

  it('renders a down beat as the down color at full strip height', () => {
    const heartbeats = [
      { ok: true, ts: 1, latency_ms: 20 },
      { ok: false, ts: 2, latency_ms: 0 },
    ]
    const w = mount(HeartbeatBar, { props: { heartbeats, size: 'md' } })
    const down = w.findAll('[data-tick]')[1]
    expect(down.classes()).toContain('bg-sev-error')
    // down = full-height spike ≈ strip height for md (28px)
    expect(parseFloat(down.element.style.height)).toBeCloseTo(28, 1)
  })

  it('flags a slow up beat amber and keeps normal up beats success-toned', () => {
    // up latencies [10,10,10,30] → median 10, slowThreshold 2.2*10 = 22, so 30 >= 22 is slow
    const heartbeats = [
      { ok: true, ts: 1, latency_ms: 10 },
      { ok: true, ts: 2, latency_ms: 10 },
      { ok: true, ts: 3, latency_ms: 10 },
      { ok: true, ts: 4, latency_ms: 30 },
    ]
    const ticks = mount(HeartbeatBar, { props: { heartbeats } }).findAll('[data-tick]')
    expect(ticks[3].classes()).toContain('bg-sev-warn')
    expect(ticks[0].classes()).toContain('bg-success')
    expect(ticks[1].classes()).toContain('bg-success')
    expect(ticks[2].classes()).toContain('bg-success')
  })

  it('scales bar height with latency for up beats', () => {
    const heartbeats = [
      { ok: true, ts: 1, latency_ms: 10 },
      { ok: true, ts: 2, latency_ms: 100 },
    ]
    const ticks = mount(HeartbeatBar, { props: { heartbeats } }).findAll('[data-tick]')
    const lo = parseFloat(ticks[0].element.style.height)
    const hi = parseFloat(ticks[1].element.style.height)
    expect(hi).toBeGreaterThan(lo)
  })

  it('shows the empty state and no ticks when there are no beats', () => {
    const w = mount(HeartbeatBar, { props: { heartbeats: [] } })
    expect(w.findAll('[data-tick]').length).toBe(0)
    expect(w.text()).toContain('no checks yet')
  })

  it('applies the container height for each size variant', () => {
    const heartbeats = [{ ok: true, ts: 1, latency_ms: 10 }]
    for (const [size, px] of [['sm', 20], ['md', 28], ['lg', 44]]) {
      const w = mount(HeartbeatBar, { props: { heartbeats, size } })
      const strip = w.find('[data-tick]').element.parentElement
      expect(strip.style.height).toBe(px + 'px')
    }
  })
})
