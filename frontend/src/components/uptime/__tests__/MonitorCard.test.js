import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
vi.mock('@/lib/uptime/uptimeQueries', () => ({
  useHeartbeats: () => ({ data: { value: { heartbeats: [{ ok: true, ts: 1, latency_ms: 10 }], uptime_pct: 99.5 } } }),
}))
import MonitorCard from '@/components/uptime/MonitorCard.vue'

const monitor = { id: 'a', name: 'Alpha', target: 'https://a.test', type: 'http', enabled: true, last_state: 'up', last_latency_ms: 12 }

describe('MonitorCard', () => {
  it('shows name, uptime and latency', () => {
    const w = mount(MonitorCard, { props: { monitor } })
    expect(w.text()).toContain('Alpha')
    expect(w.text()).toMatch(/99\.5/)
    expect(w.text()).toContain('12 ms')
  })
  it('emits select with the monitor id on click', async () => {
    const w = mount(MonitorCard, { props: { monitor } })
    await w.trigger('click')
    expect(w.emitted('select')?.[0]).toEqual(['a'])
  })
})
