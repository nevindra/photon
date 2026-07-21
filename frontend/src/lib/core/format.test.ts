import { describe, it, expect } from 'vitest'
import { formatRate } from './format'

describe('formatRate', () => {
  it('formats byte rates compactly', () => {
    expect(formatRate(512)).toBe('512 B/s')
    expect(formatRate(2_150_000)).toBe('2.1 MB/s')
  })
  it('dashes null/undefined/NaN', () => {
    expect(formatRate(null)).toBe('—')
    expect(formatRate(undefined)).toBe('—')
    expect(formatRate(Number.NaN)).toBe('—')
  })
})
