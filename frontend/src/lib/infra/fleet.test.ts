import { describe, it, expect } from 'vitest'
import { hostVerdict, fleetRows, fleetSummary } from './fleet'
import type { InfraHost, ManagedHost } from '@/lib/core/api'

function host(over: Partial<InfraHost> = {}): InfraHost {
  return { host: 'h', cpuUtil: 0.3, memUtil: 0.4, diskUtil: 0.35, diskUtilAvg: 0.35, diskGroups: 1, gpuUtil: null, gpuUtilAvg: null, gpuGroups: 0, lastSeenNs: '0', hasGpu: false, ...over }
}
function agent(over: Partial<ManagedHost> = {}): ManagedHost {
  return { id: 'a', host: 'h', site: null, status: 'online', agent_version: '1.7.0', latest_agent_version: null, capabilities: ['read'], poll_secs: 15, last_seen_ms: Date.now(), enrolled_ms: 0, docker: null, labels: {}, ...over }
}

describe('hostVerdict', () => {
  it('names the single worst resource in words, error before warning, then the higher value', () => {
    expect(hostVerdict(host({ cpuUtil: 0.85, diskUtil: 0.95 }))).toEqual({ label: 'disk 95%', accent: 'error', detail: null })
    expect(hostVerdict(host({ cpuUtil: 0.85, memUtil: 0.88 }))).toEqual({ label: 'memory 88%', accent: 'warning', detail: null })
    expect(hostVerdict(host())).toBeNull()
  })

  it('carries the across-group mean for a split resource with several groups', () => {
    const v = hostVerdict(host({ diskUtil: 0.9, diskUtilAvg: 0.61, diskGroups: 3 }))
    expect(v?.label).toBe('disk 90%')
    expect(v?.detail).toBe('worst of 3 mounts · average 61%')
    expect(hostVerdict(host({ diskUtil: 0.85, diskUtilAvg: 0.85, diskGroups: 1 }))?.detail).toBeNull()
    expect(hostVerdict(host({ hasGpu: true, gpuUtil: 0.92, gpuUtilAvg: 0.4, gpuGroups: 2 }))?.detail).toBe('worst of 2 devices · average 40%')
  })
})

describe('fleetRows', () => {
  it('unions metrics and agents into one row per host, ranked worst first', () => {
    const rows = fleetRows(
      [host({ host: 'fine' }), host({ host: 'hot', diskUtil: 0.95 }), host({ host: 'warm', memUtil: 0.85 }), host({ host: 'quiet' })],
      [agent({ host: 'fine' }), agent({ host: 'new', status: 'pending', last_seen_ms: null }), agent({ host: 'quiet', status: 'offline' })],
    )
    expect(rows.map((r) => [r.host, r.level])).toEqual([
      ['hot', 'error'],
      ['quiet', 'offline'],
      ['warm', 'warning'],
      ['new', 'pending'],
      ['fine', 'ok'],
    ])
    expect(rows.find((r) => r.host === 'fine')?.agent?.status).toBe('online')
    expect(rows.find((r) => r.host === 'new')?.metrics).toBeNull()
    expect(rows.find((r) => r.host === 'hot')?.verdict?.label).toBe('disk 95%')
  })

  it('a pending agent whose metrics already arrive is judged on the metrics', () => {
    const rows = fleetRows([host({ host: 'h' })], [agent({ host: 'h', status: 'pending' })])
    expect(rows[0].level).toBe('ok')
  })
})

describe('fleetSummary', () => {
  it('counts every level once, averages CPU over reporters, and sums containers over managed hosts', () => {
    const rows = fleetRows(
      [host({ host: 'a', cpuUtil: 0.2 }), host({ host: 'b', cpuUtil: 0.6, diskUtil: 0.95, hasGpu: true, gpuUtil: 0.1 }), host({ host: 'c', cpuUtil: null })],
      [
        agent({ host: 'a', docker: { version: '27', api_version: '1.46', storage_driver: 'overlay2', os: 'Ubuntu', arch: 'x86_64', containers_running: 2, containers_total: 2, images: 3, disk: { images_bytes: 0, volumes_bytes: 0, build_cache_bytes: 0, reclaimable_bytes: 0, total_bytes: null, used_bytes: null } } }),
        agent({ host: 'd', status: 'pending', last_seen_ms: null }),
      ],
    )
    const s = fleetSummary(rows)
    expect(s.total).toBe(4)
    expect(s.counts).toEqual({ error: 1, offline: 0, warning: 0, pending: 1, ok: 2 })
    expect(s.avgCpu).toBeCloseTo(0.4)
    expect(s.gpuHosts).toBe(1)
    expect(s.containers).toEqual({ running: 2, total: 2 })
    expect(fleetSummary([]).avgCpu).toBeNull()
    expect(fleetSummary([]).containers).toBeNull()
  })
})
