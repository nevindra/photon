import { describe, it, expect } from 'vitest'
import { judge, judgeAll, isDegraded, STALLED_FAILURES } from '@/lib/data/compaction'
import type { CompactionHealth } from '@/lib/core/api'

const NOW = 1_751_000_000_000

function h(over: Partial<CompactionHealth> = {}): CompactionHealth {
  return { signal: 'logs', last_ok_ms: NOW - 20_000, consecutive_failures: 0, last_error: null, ...over }
}

describe('compaction judgment', () => {
  it('a merging signal is healthy and says when it last merged', () => {
    const v = judge(h(), NOW)
    expect(v.tone).toBe('success')
    expect(v.label).toBe('Merging')
    expect(v.detail).toBe('Last merged 20s ago')
    expect(isDegraded(v)).toBe(false)
  })

  it('a node that has not merged yet is unknown, not healthy', () => {
    const v = judge(h({ last_ok_ms: null }), NOW)
    expect(v.tone).toBe('neutral')
    expect(v.label).toBe('Unknown')
  })

  it('one failed pass already warns', () => {
    const v = judge(h({ consecutive_failures: 1, last_error: 'boom' }), NOW)
    expect(v.tone).toBe('warning')
    expect(v.label).toBe('Failing')
    expect(v.detail).toBe('1 failed pass in a row, stalled since 20s ago')
    expect(isDegraded(v)).toBe(true)
  })

  it('a sustained streak escalates to stalled', () => {
    const v = judge(h({ consecutive_failures: STALLED_FAILURES }), NOW)
    expect(v.tone).toBe('error')
    expect(v.label).toBe('Stalled')
  })

  it('reports a long stall in days, the unit the failure actually happens in', () => {
    const v = judge(h({ consecutive_failures: 103_680, last_ok_ms: NOW - 12 * 86_400_000 }), NOW)
    expect(v.detail).toContain('12d ago')
  })

  it('a stall that started before any success does not invent a "since"', () => {
    const v = judge(h({ consecutive_failures: 5, last_ok_ms: null }), NOW)
    expect(v.detail).toBe('5 failed passes in a row, never merged since start-up')
  })

  it('no compactor attached reports nothing rather than three healthy signals', () => {
    expect(judgeAll(undefined, NOW)).toEqual([])
  })

  it('judges each signal independently', () => {
    const out = judgeAll(
      [h({ signal: 'logs' }), h({ signal: 'traces', consecutive_failures: 97 })],
      NOW,
    )
    expect(out.map((v) => v.tone)).toEqual(['success', 'error'])
    expect(out.filter(isDegraded).map((v) => v.signal)).toEqual(['traces'])
  })
})
