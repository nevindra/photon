import { describe, it, expect } from 'vitest'
import {
  serviceStatus, healthReasons, healthCounts, byWorstFirst, attentionServices, STATUS_META,
} from '@/lib/services/serviceHealth'

describe('serviceStatus', () => {
  it('is idle with no traffic', () => {
    expect(serviceStatus({ count: 0, error_rate: 0.9, apdex: 0.1 })).toBe('idle')
  })
  it('is critical at/above the error threshold', () => {
    expect(serviceStatus({ count: 10, error_rate: 0.05, apdex: 0.99 })).toBe('critical')
  })
  it('is critical when apdex is very low even with no errors', () => {
    expect(serviceStatus({ count: 10, error_rate: 0, apdex: 0.69 })).toBe('critical')
  })
  it('is degraded in the middle band', () => {
    expect(serviceStatus({ count: 10, error_rate: 0.01, apdex: 0.99 })).toBe('degraded')
    expect(serviceStatus({ count: 10, error_rate: 0, apdex: 0.84 })).toBe('degraded')
  })
  it('is healthy when clean', () => {
    expect(serviceStatus({ count: 10, error_rate: 0.001, apdex: 0.98 })).toBe('healthy')
  })
  it('ignores null apdex (falls back to error rate only)', () => {
    expect(serviceStatus({ count: 10, error_rate: 0.001, apdex: null })).toBe('healthy')
    expect(serviceStatus({ count: 10, error_rate: 0.2, apdex: null })).toBe('critical')
  })
})

describe('healthReasons', () => {
  it('names the crossed conditions', () => {
    expect(healthReasons({ error_rate: 0.082, apdex: 0.71 })).toEqual(['Error rate 8.2%', 'Apdex 0.71'])
  })
  it('is empty when healthy', () => {
    expect(healthReasons({ error_rate: 0.001, apdex: 0.99 })).toEqual([])
  })
})

describe('healthCounts', () => {
  it('tallies by status', () => {
    const rows = [
      { count: 0 },
      { count: 5, error_rate: 0.06, apdex: 0.9 },
      { count: 5, error_rate: 0.02, apdex: 0.9 },
      { count: 5, error_rate: 0, apdex: 0.99 },
    ]
    expect(healthCounts(rows)).toEqual({ critical: 1, degraded: 1, healthy: 1, idle: 1 })
  })
})

describe('byWorstFirst / attentionServices', () => {
  const rows = [
    { service: 'ok', count: 100, error_rate: 0, apdex: 0.99 },
    { service: 'crit', count: 100, error_rate: 0.2, apdex: 0.5 },
    { service: 'degr', count: 100, error_rate: 0.02, apdex: 0.9 },
    { service: 'idle', count: 0 },
  ]
  it('orders critical → degraded → healthy → idle', () => {
    expect(byWorstFirst(rows).map((r) => r.service)).toEqual(['crit', 'degr', 'ok', 'idle'])
  })
  it('picks only non-healthy, worst-first, capped', () => {
    expect(attentionServices(rows, 3).map((r) => r.service)).toEqual(['crit', 'degr'])
    expect(attentionServices(rows, 1).map((r) => r.service)).toEqual(['crit'])
  })
})

describe('STATUS_META', () => {
  it('has literal class strings for every status', () => {
    for (const k of ['critical', 'degraded', 'healthy', 'idle']) {
      expect(typeof STATUS_META[k].dot).toBe('string')
      expect(typeof STATUS_META[k].label).toBe('string')
    }
  })
})
