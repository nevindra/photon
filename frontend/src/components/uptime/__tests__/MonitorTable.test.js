import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
vi.mock('@/lib/uptime/uptimeQueries', () => ({
  useHeartbeats: () => ({ data: { value: { heartbeats: [{ ok: true, ts: 1, latency_ms: 10 }], uptime_pct: 99.5 } } }),
}))
import MonitorTable from '@/components/uptime/MonitorTable.vue'

const monitor = { id: 'a', name: 'Alpha', target: 'https://a.test', type: 'http', enabled: true, last_state: 'up', last_latency_ms: 12 }

describe('MonitorTable', () => {
  it('renders a row per monitor', () => {
    const w = mount(MonitorTable, { props: { monitors: [monitor, { ...monitor, id: 'b', name: 'Bravo' }] } })
    expect(w.text()).toContain('Alpha')
    expect(w.text()).toContain('Bravo')
  })
  it('emits select with the monitor id when a row is clicked', async () => {
    const w = mount(MonitorTable, { props: { monitors: [monitor, { ...monitor, id: 'b', name: 'Bravo' }] } })
    await w.find('tbody tr').trigger('click')
    expect(w.emitted('select')?.[0]).toEqual(['a'])
  })
})
