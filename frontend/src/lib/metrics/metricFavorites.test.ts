import { describe, it, expect } from 'vitest'
import { createMetricFavorites } from '@/lib/metrics/metricFavorites'

// Minimal in-memory Storage fake (only the two methods we use).
function fakeStorage(): Storage {
  const m = new Map<string, string>()
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
    clear: () => m.clear(),
    key: () => null,
    get length() { return m.size },
  } as Storage
}

describe('createMetricFavorites', () => {
  it('toggles a favorite on and off and persists to storage', () => {
    const s = fakeStorage()
    const f = createMetricFavorites(s)
    f.toggleFavorite('cpu.usage')
    expect(f.isFavorite('cpu.usage')).toBe(true)
    expect(JSON.parse(s.getItem('photon.metrics.favorites')!)).toEqual(['cpu.usage'])
    f.toggleFavorite('cpu.usage')
    expect(f.isFavorite('cpu.usage')).toBe(false)
  })

  it('records recent most-recent-first, de-duplicated, capped at 8', () => {
    const f = createMetricFavorites(fakeStorage())
    for (let i = 0; i < 10; i++) f.recordRecent('m' + i)
    f.recordRecent('m9') // re-touch moves to front, no dupe
    expect(f.recent.value.length).toBe(8)
    expect(f.recent.value[0]).toBe('m9')
    expect(new Set(f.recent.value).size).toBe(8)
  })

  it('hydrates from existing storage', () => {
    const s = fakeStorage()
    s.setItem('photon.metrics.favorites', JSON.stringify(['a']))
    expect(createMetricFavorites(s).favorites.value).toEqual(['a'])
  })

  it('never throws when storage is blocked or corrupt', () => {
    const blocked = {
      getItem: () => 'not json{',
      setItem: () => { throw new Error('QuotaExceeded') },
    } as unknown as Storage
    const f = createMetricFavorites(blocked)
    expect(f.favorites.value).toEqual([]) // corrupt JSON → []
    expect(() => f.toggleFavorite('x')).not.toThrow()
    expect(f.isFavorite('x')).toBe(true) // in-memory value still updates
  })
})
