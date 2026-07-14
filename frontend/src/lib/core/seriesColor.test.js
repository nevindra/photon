// frontend/src/lib/seriesColor.test.js
import { describe, it, expect } from 'vitest'
import { seriesColor, seriesLabelKey, SERIES_PALETTE } from '@/lib/core/seriesColor'
import { serviceColorClass } from '@/lib/services/serviceColor'

describe('seriesColor', () => {
  it('returns a stable stroke+swatch for a key', () => {
    const a = seriesColor('checkout')
    const b = seriesColor('checkout')
    expect(a).toEqual(b)
    expect(a.stroke).toMatch(/^#[0-9a-f]{6}$/i)
    expect(SERIES_PALETTE).toHaveLength(10)
  })
  it('matches serviceColor hue assignment (so service-grouped lines match the waterfall)', () => {
    for (const name of ['checkout', 'cart', 'payment', 'api-gateway']) {
      expect(seriesColor(name).swatch).toBe(serviceColorClass(name))
    }
  })
  it('handles empty/nullish keys without throwing', () => {
    expect(() => seriesColor('')).not.toThrow()
    expect(() => seriesColor(null)).not.toThrow()
  })
})

describe('seriesLabelKey', () => {
  it('joins a single label as k=v', () => {
    expect(seriesLabelKey({ service: 'checkout' })).toBe('service=checkout')
  })
  it('joins multiple labels with ", "', () => {
    expect(seriesLabelKey({ service: 'checkout', region: 'us' })).toBe('service=checkout, region=us')
  })
  it("falls back to '(all)' for empty/nullish labels", () => {
    expect(seriesLabelKey({})).toBe('(all)')
    expect(seriesLabelKey(null)).toBe('(all)')
    expect(seriesLabelKey(undefined)).toBe('(all)')
  })
})
