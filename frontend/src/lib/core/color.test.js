import { describe, it, expect } from 'vitest'
import { parseHslTriplet, hslToRgb, flattenHsl } from '@/lib/core/color'

describe('parseHslTriplet', () => {
  it('parses a shadcn "H S% L%" triplet', () => {
    expect(parseHslTriplet('0 0% 45.1%')).toEqual({ h: 0, s: 0, l: 45.1 })
    expect(parseHslTriplet('  262 83% 58%  ')).toEqual({ h: 262, s: 83, l: 58 })
  })

  it('returns null for anything that is not a plain triplet', () => {
    expect(parseHslTriplet('hsl(0 0% 45%)')).toBeNull()
    expect(parseHslTriplet('rebeccapurple')).toBeNull()
    expect(parseHslTriplet('')).toBeNull()
    expect(parseHslTriplet(null)).toBeNull()
  })
})

describe('hslToRgb', () => {
  it('maps a greyscale colour to equal channels (r=g=b=round(255·l/100))', () => {
    expect(hslToRgb(0, 0, 100)).toEqual({ r: 255, g: 255, b: 255 })
    expect(hslToRgb(0, 0, 0)).toEqual({ r: 0, g: 0, b: 0 })
    expect(hslToRgb(0, 0, 45.1)).toEqual({ r: 115, g: 115, b: 115 })
    expect(hslToRgb(0, 0, 3.9)).toEqual({ r: 10, g: 10, b: 10 })
  })

  it('handles a saturated hue (pure red)', () => {
    expect(hslToRgb(0, 100, 50)).toEqual({ r: 255, g: 0, b: 0 })
  })
})

describe('flattenHsl', () => {
  it('flattens 35% muted-fg over the white card into an opaque light grey (light theme)', () => {
    // 115·0.35 + 255·0.65 = 206 on every channel.
    expect(flattenHsl('0 0% 45.1%', '0 0% 100%', 0.35)).toBe('rgb(206, 206, 206)')
  })

  it('flattens 35% muted-fg over the near-black card into an opaque dim grey (dark theme)', () => {
    // muted-fg 63.9% → 163, card 3.9% → 10; 163·0.35 + 10·0.65 = 63.55 → 64.
    expect(flattenHsl('0 0% 63.9%', '0 0% 3.9%', 0.35)).toBe('rgb(64, 64, 64)')
  })

  it('is fully opaque at alpha 1 (returns the foreground) and equals the bg at alpha 0', () => {
    expect(flattenHsl('0 0% 45.1%', '0 0% 100%', 1)).toBe('rgb(115, 115, 115)')
    expect(flattenHsl('0 0% 45.1%', '0 0% 100%', 0)).toBe('rgb(255, 255, 255)')
  })

  it('falls back to a plain hsl() string when a triplet is unparseable', () => {
    expect(flattenHsl('not-a-triplet', '0 0% 100%', 0.35)).toBe('hsl(not-a-triplet)')
  })
})
