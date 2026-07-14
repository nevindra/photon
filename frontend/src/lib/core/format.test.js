import { describe, it, expect } from 'vitest'
import {
  SEVERITIES,
  severity,
  toneClasses,
  severityClasses,
  formatClock,
  formatNumber,
  formatBytes,
  formatDuration,
} from '@/lib/core/format'

describe('severity model', () => {
  it('orders debug→fatal and marks only warn/error/fatal as coloured', () => {
    expect(SEVERITIES.map((s) => s.key)).toEqual(['debug', 'info', 'warn', 'error', 'fatal'])
    expect(severity('debug').tone).toBe('neutral')
    expect(severity('info').tone).toBe('neutral')
    expect(severity('warn').tone).toBe('warn')
    expect(severity('error').tone).toBe('error')
    expect(severity('fatal').tone).toBe('fatal')
  })

  it('falls back to info for unknown keys', () => {
    expect(severity('nope').key).toBe('info')
  })

  it('maps tones to full literal Tailwind classes', () => {
    expect(toneClasses('error')).toEqual({
      text: 'text-sev-error',
      bgSoft: 'bg-sev-error-soft',
      solid: 'bg-sev-error',
    })
    expect(toneClasses('neutral').text).toBe('text-muted-foreground')
    // unknown tone → neutral
    expect(toneClasses('???')).toEqual(toneClasses('neutral'))
  })

  it('severityClasses resolves a key straight to its tone classes', () => {
    expect(severityClasses('fatal')).toEqual(toneClasses('fatal'))
    expect(severityClasses('info')).toEqual(toneClasses('neutral'))
  })
})

describe('formatters', () => {
  it('formats epoch nanos (BigInt) as a clock with millis', () => {
    // 1970-01-01T00:00:01.234Z in local time — assert the millis + structure
    const nanos = 1_234n * 1_000_000n
    expect(formatClock(nanos)).toMatch(/^\d{2}:\d{2}:\d{2}\.234$/)
  })

  it('formats numbers with thousands separators', () => {
    expect(formatNumber(18204)).toBe('18,204')
  })

  it('formats bytes with binary units', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(210_000_000)).toBe('200.3 MB')
    expect(formatBytes(64_000_000)).toBe('61.0 MB')
    expect(formatBytes(2 * 1024 ** 3)).toBe('2.0 GB')
    expect(formatBytes(null)).toBe('—')
  })
})

describe('formatDuration', () => {
  it('renders nanoseconds', () => {
    expect(formatDuration(0)).toBe('0ns')
    expect(formatDuration(999)).toBe('999ns')
  })
  it('renders microseconds', () => {
    expect(formatDuration(1_500)).toBe('1.5µs')
  })
  it('renders milliseconds', () => {
    expect(formatDuration(2_500_000)).toBe('2.5ms')
  })
  it('renders seconds', () => {
    expect(formatDuration(1_500_000_000)).toBe('1.50s')
  })
  it('accepts BigInt', () => {
    expect(formatDuration(2_500_000n)).toBe('2.5ms')
  })
  it('handles null/undefined', () => {
    expect(formatDuration(null)).toBe('—')
    expect(formatDuration(undefined)).toBe('—')
  })
})
