import { describe, it, expect } from 'vitest'
import { actionLabel, targetSummary, sinceMs, ageMs } from './audit'
import type { AuditEntry } from '@/lib/core/api'

describe('audit display helpers', () => {
  it('turns noun.verb into a phrase', () => {
    expect(actionLabel('container.restart')).toBe('Restart container')
    expect(actionLabel('stack.apply')).toBe('Apply stack')
    expect(actionLabel('agent.rotate_token')).toBe('Rotate token for agent')
    expect(actionLabel('rollout.rollback')).toBe('Roll back rollout')
    expect(actionLabel('prune')).toBe('Prune')
    expect(actionLabel('thing.frobnicate')).toBe('Frobnicate thing')
  })

  it('summarises the most specific target and its host', () => {
    const e = { target: { host: 'web-1', container: 'api' } } as AuditEntry
    expect(targetSummary(e)).toEqual({ primary: 'api', host: 'web-1' })
    const t = { target: { template: 'nodeflux-vas', version: '2.4.1' } } as AuditEntry
    expect(targetSummary(t)).toEqual({ primary: 'nodeflux-vas 2.4.1', host: null })
    expect(targetSummary({ target: {} } as AuditEntry)).toEqual({ primary: null, host: null })
  })

  it('resolves the since picker to an absolute cutoff, and "all" to none', () => {
    const now = 1_000_000_000_000
    expect(sinceMs('24h', now)).toBe(now - 24 * 3_600_000)
    expect(sinceMs('7d', now)).toBe(now - 7 * 24 * 3_600_000)
    expect(sinceMs('all', now)).toBeUndefined()
    expect(sinceMs('bogus', now)).toBe(now - 7 * 24 * 3_600_000)
  })

  it('renders ages in the coarsest sensible unit', () => {
    const now = 1_000_000_000_000
    expect(ageMs(now - 9_000, now)).toBe('9s ago')
    expect(ageMs(now - 5 * 60_000, now)).toBe('5m ago')
    expect(ageMs(now - 7 * 3_600_000, now)).toBe('7h ago')
    expect(ageMs(now - 3 * 86_400_000, now)).toBe('3d ago')
  })
})
