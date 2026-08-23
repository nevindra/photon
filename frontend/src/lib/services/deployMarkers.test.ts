import { describe, it, expect } from 'vitest'
import { deployMarkers, MAX_MARKERS } from './deployMarkers'
import type { Deployment } from '@/lib/core/api'

const T0 = 1_700_000_000_000

function deploy(version: string, at: number): Deployment {
  return {
    id: `api-${version}`,
    service: 'api',
    environment: 'prod',
    tenant: '',
    version,
    detected_at_ms: at,
    source: 'detected',
    commit_sha: null,
    author: null,
    message: null,
    url: null,
  }
}

describe('deployMarkers', () => {
  it('labels a marker with the version and puts it at the detection time', () => {
    expect(deployMarkers([deploy('1.4.2', T0)])).toEqual([
      { xMs: T0, label: '1.4.2', color: 'hsl(var(--brand))' },
    ])
  })

  it('keeps the newest releases when there are too many to read', () => {
    const many = Array.from({ length: MAX_MARKERS + 5 }, (_, i) => deploy(`1.0.${i}`, T0 + i * 60_000))
    const out = deployMarkers(many)
    expect(out).toHaveLength(MAX_MARKERS)
    // Newest first, and the oldest five are the ones dropped.
    expect(out[0].label).toBe(`1.0.${MAX_MARKERS + 4}`)
    expect(out.map((m) => m.label)).not.toContain('1.0.0')
  })

  it('does not mutate the array it was handed', () => {
    const list = [deploy('1.0.0', T0), deploy('1.1.0', T0 - 1000)]
    deployMarkers(list)
    expect(list.map((d) => d.version)).toEqual(['1.0.0', '1.1.0'])
  })

  it('returns nothing for no releases', () => {
    expect(deployMarkers([])).toEqual([])
  })
})
