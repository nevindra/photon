import { describe, it, expect, vi, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
vi.mock('@/lib/uptime/uptimeQueries', () => ({
  useMonitor: () => ({ data: { value: { id: 'a', name: 'Alpha', target: 'https://a.test', type: 'http', enabled: true, last_state: 'up', last_latency_ms: 12 } } }),
  useHeartbeats: () => ({ data: { value: { heartbeats: [{ ok: true, ts: 1, latency_ms: 10 }], uptime_pct: 99.5 } } }),
  useIncidents: () => ({ data: { value: [{ id: 1, started_at: 1000, ended_at: 2000, cause: 'boom' }] } }),
  useUpdateMonitor: () => ({ mutate: vi.fn() }),
  useDeleteMonitor: () => ({ mutate: vi.fn() }),
  usePauseMonitor: () => ({ mutate: vi.fn() }),
  useResumeMonitor: () => ({ mutate: vi.fn() }),
}))
import MonitorDetailDialog from '@/components/uptime/MonitorDetailDialog.vue'

afterEach(() => { document.body.innerHTML = '' })

describe('MonitorDetailDialog', () => {
  it('shows name, uptime and an incident when open', async () => {
    mount(MonitorDetailDialog, { props: { monitorId: 'a', open: true }, attachTo: document.body,
      global: { stubs: { HeartbeatBar: true, MonitorForm: true } } })
    await new Promise((r) => setTimeout(r))
    expect(document.body.textContent).toContain('Alpha')
    expect(document.body.textContent).toMatch(/99\.5/)
    expect(document.body.textContent).toContain('boom')
  })

  it('deletes and requests close', async () => {
    const w = mount(MonitorDetailDialog, { props: { monitorId: 'a', open: true }, attachTo: document.body,
      global: { stubs: { HeartbeatBar: true, MonitorForm: true } } })
    await new Promise((r) => setTimeout(r))
    const del = [...document.body.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Delete')
    expect(del).toBeTruthy()
    del.click()
    await new Promise((r) => setTimeout(r))
    expect(w.emitted('update:open')?.at(-1)).toEqual([false])
  })
})
