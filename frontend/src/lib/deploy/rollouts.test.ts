import { describe, it, expect } from 'vitest'
import { resolveTargets, canarySize, planStages, targetCounts, selectorSummary, strategySummary, availableRolloutActions, DEFAULT_STRATEGY } from './rollouts'
import type { ManagedHost } from '@/lib/core/api'

const host = (h: string, over: Partial<ManagedHost> = {}): ManagedHost => ({ id: h, host: h, site: null, status: 'online', agent_version: '1', latest_agent_version: '1', capabilities: ['read', 'lifecycle', 'apply'], poll_secs: 15, last_seen_ms: 0, enrolled_ms: 0, docker: null, labels: {}, ...over })
const HOSTS = [host('a', { site: 's1', labels: { role: 'vas' } }), host('b', { site: 's1', status: 'offline', labels: { role: 'vas' } }), host('c', { site: 's2', capabilities: ['read'] }), host('d', { labels: { role: 'web' } })]

describe('rollouts', () => {
  it('resolves hosts, sites and labels, noting offline and no-apply', () => {
    expect(resolveTargets({ kind: 'hosts', hosts: ['a', 'd'] }, HOSTS).map((t) => t.host)).toEqual(['d', 'a'])
    const site = resolveTargets({ kind: 'site', sites: ['s1'] }, HOSTS)
    expect(site.map((t) => [t.host, t.eligible, t.note])).toEqual([['a', true, null], ['b', true, 'offline — waits as pending']])
    const c = resolveTargets({ kind: 'site', sites: ['s2'] }, HOSTS)[0]
    expect(c.eligible).toBe(false)
    expect(resolveTargets({ kind: 'labels', labels: { role: 'vas' } }, HOSTS).map((t) => t.host)).toEqual(['a', 'b'])
    expect(resolveTargets({ kind: 'labels', labels: {} }, HOSTS)).toHaveLength(4)
  })

  it('sizes the canary and prefers online hosts for it', () => {
    expect(canarySize(10, { canary: 20, canary_is_percent: true })).toBe(2)
    expect(canarySize(3, { canary: 5, canary_is_percent: false })).toBe(3)
    expect(canarySize(0, DEFAULT_STRATEGY)).toBe(0)
    const plan = planStages(resolveTargets({ kind: 'site', sites: ['s1'] }, HOSTS).reverse(), DEFAULT_STRATEGY)
    expect(plan.find((p) => p.host === 'a')?.stage).toBe(1)
    expect(plan.find((p) => p.host === 'b')?.stage).toBe(2)
    expect(planStages(resolveTargets({ kind: 'site', sites: ['s2'] }, HOSTS), DEFAULT_STRATEGY)[0].stage).toBe(0)
  })

  it('counts targets and summarises selector and strategy', () => {
    expect(targetCounts([{ status: 'healthy' }, { status: 'failed' }, { status: 'pending' }, { status: 'applying' }, { status: 'skipped' }])).toEqual({ total: 5, healthy: 1, failed: 1, pending: 1, queued: 0, inflight: 1, skipped: 1, drifted: 0 })
    expect(selectorSummary({ kind: 'site', sites: ['divtik'] })).toBe('site divtik')
    expect(selectorSummary({ kind: 'labels', labels: { role: 'vas' } })).toBe('labels role=vas')
    expect(strategySummary(DEFAULT_STRATEGY)).toContain('1 canary')
    expect(strategySummary(DEFAULT_STRATEGY)).toContain('pre-pull')
  })

  it('offers the verbs a state allows', () => {
    expect(availableRolloutActions({ state: 'draft', stage: 1, stages: 2, previous_version: null })).toEqual(['start', 'abort'])
    expect(availableRolloutActions({ state: 'paused_gate', stage: 1, stages: 2, previous_version: '1' })).toEqual(['promote', 'abort', 'rollback'])
    expect(availableRolloutActions({ state: 'paused_gate', stage: 2, stages: 2, previous_version: null })).toEqual(['abort'])
    expect(availableRolloutActions({ state: 'paused_threshold', stage: 1, stages: 2, previous_version: '1' })).toEqual(['retry', 'resume', 'abort', 'rollback'])
    expect(availableRolloutActions({ state: 'rolled_back', stage: 2, stages: 2, previous_version: '1' })).toEqual([])
  })
})
