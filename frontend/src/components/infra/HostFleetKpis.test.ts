// Fleet KPI band above the /infra host card grid. Asserts the derived StatTile props directly
// (findAllComponents + props, same pattern as RumVitalsView.test.js's WebVitalScorecard check)
// rather than scraping rendered text — Warning/Critical/GPU-hosts counts can coincide numerically
// (e.g. all "1"), so text-based assertions can't tell the tiles apart, but the props feeding each
// StatTile can.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HostFleetKpis from './HostFleetKpis.vue'
import { StatTile } from '@/components/ui/stat-tile'
import type { InfraHost } from '@/lib/core/api'

function host(overrides: Partial<InfraHost> = {}): InfraHost {
  return {
    host: 'h',
    cpuUtil: 0.3,
    memUtil: 0.4,
    diskUtil: 0.35,
    gpuUtil: null,
    lastSeenNs: '0',
    hasGpu: false,
    ...overrides,
  }
}

function tilesByLabel(hosts: InfraHost[]) {
  const w = mount(HostFleetKpis, { props: { hosts } })
  const tiles = w.findAllComponents(StatTile)
  return Object.fromEntries(tiles.map((t) => [t.props('label'), t]))
}

describe('HostFleetKpis', () => {
  it('derives Hosts/Warning/Critical/Avg CPU/GPU hosts from a 4-host fixture, no double-count', () => {
    const hosts: InfraHost[] = [
      host({ host: 'healthy-1', cpuUtil: 0.3, memUtil: 0.4, diskUtil: 0.35 }),
      host({ host: 'warning-1', cpuUtil: 0.85, memUtil: 0.5, diskUtil: 0.4 }),
      host({ host: 'critical-1', cpuUtil: 0.5, memUtil: 0.4, diskUtil: 0.95 }),
      host({ host: 'gpu-1', cpuUtil: null, memUtil: 0.6, diskUtil: 0.5, gpuUtil: 0.4, hasGpu: true }),
    ]
    const tiles = tilesByLabel(hosts)

    expect(tiles['Hosts'].props('value')).toBe(4)

    // The critical host (diskUtil 0.95) must count only toward Critical, never also Warning.
    expect(tiles['Warning'].props('value')).toBe(1)
    expect(tiles['Warning'].props('accent')).toBe('warning')
    expect(tiles['Critical'].props('value')).toBe(1)
    expect(tiles['Critical'].props('accent')).toBe('error')

    // Mean of non-null cpuUtil only: (0.3 + 0.85 + 0.5) / 3 = 0.55 — the GPU host's null cpuUtil
    // is excluded, not treated as 0.
    expect(tiles['Avg CPU'].props('value')).toBe('55%')

    expect(tiles['GPU hosts'].props('value')).toBe(1)
  })

  it('shows no warning/critical accent and a dash avg CPU for an all-healthy, CPU-less fleet', () => {
    const hosts: InfraHost[] = [
      host({ host: 'a', cpuUtil: null, memUtil: 0.2, diskUtil: 0.2 }),
      host({ host: 'b', cpuUtil: null, memUtil: 0.3, diskUtil: 0.3 }),
    ]
    const tiles = tilesByLabel(hosts)
    expect(tiles['Hosts'].props('value')).toBe(2)
    expect(tiles['Warning'].props('value')).toBe(0)
    expect(tiles['Warning'].props('accent')).toBeUndefined()
    expect(tiles['Critical'].props('value')).toBe(0)
    expect(tiles['Critical'].props('accent')).toBeUndefined()
    expect(tiles['Avg CPU'].props('value')).toBe('—')
    expect(tiles['GPU hosts'].props('value')).toBe(0)
  })
})
