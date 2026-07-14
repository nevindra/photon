// frontend/src/lib/signalMeta.test.ts
import { describe, it, expect } from 'vitest'
import { signalColor, signalIcon, signalMeta } from '@/lib/core/signalMeta'

describe('signalColor', () => {
  it('returns a stable hex color for the same key', () => {
    const a = signalColor('logs')
    const b = signalColor('logs')
    expect(a).toBe(b)
    expect(a).toMatch(/^#[0-9a-f]{6}$/i)
  })

  it('gives known signals hash-derived hex colors', () => {
    expect(signalColor('traces')).toMatch(/^#[0-9a-f]{6}$/i)
    expect(signalColor('metrics')).toMatch(/^#[0-9a-f]{6}$/i)
  })
})

describe('signalIcon', () => {
  it('returns a defined component for each known signal', () => {
    for (const key of ['logs', 'traces', 'metrics', 'uptime']) {
      expect(signalIcon(key)).toBeDefined()
    }
  })

  it('falls back to a defined component for an unknown key', () => {
    expect(signalIcon('bogus-signal')).toBeDefined()
  })
})

describe('signalMeta', () => {
  it('bundles color + icon for a known key', () => {
    const meta = signalMeta('uptime')
    expect(meta.color).toMatch(/^#[0-9a-f]{6}$/i)
    expect(meta.icon).toBeDefined()
  })
})
