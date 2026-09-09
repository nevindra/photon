import { describe, it, expect } from 'vitest'
import { stackSubject, rolloutSubject, boardWorlds } from './attention'
import type { Rollout, Stack } from '@/lib/core/api'

const stack = (over: Partial<Stack>): Stack => ({
  id: 'st', name: 'demo', host: 'web-2', site: null, source: 'template', template_id: 't', template_name: 'T', template_version: '1', status: 'running',
  desired_generation: 1, applied_generation: 1, files: [], variables: {}, secrets: [], deploy_options: { pull: 'missing', force_recreate: false, recreate_volumes: false },
  services: [], drift: [], apm_services: [], acknowledged_warnings: [], last_error: null, created_by: 'r', created_ms: 0, updated_ms: 5, last_applied_ms: null, ...over,
})
const rollout = (over: Partial<Rollout>): Rollout => ({
  id: 'ro', name: 'VAS 2 → divtik', template_id: 't', template_name: 'T', template_version_id: 'tv', template_version: '2', previous_version: '1', stack_name: 's',
  selector: { kind: 'site', sites: ['divtik'] }, strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 },
  state: 'running', stage: 1, stages: 2, targets: [
    { host: 'a', site: 'divtik', stage: 1, status: 'failed', attempts: 1, last_error: 'x', stack_id: null, previous_version: '1', agent_status: 'online', updated_ms: 0 },
    { host: 'b', site: 'divtik', stage: 2, status: 'pending', attempts: 0, last_error: null, stack_id: null, previous_version: '1', agent_status: 'offline', updated_ms: 0 },
  ], variables: {}, created_by: 'r', created_ms: 0, updated_ms: 9, started_ms: 0, finished_ms: null, ...over,
})

describe('Deploy subjects on the Attention Board', () => {
  it('a failed stack is Critical, a drifted one Warning, a running one OK, a gone one absent', () => {
    expect(stackSubject(stack({ status: 'failed', last_error: 'port taken' }))).toMatchObject({ kind: 'stack', world: 'deploy', level: 'critical', name: 'demo @ web-2', dest: '/deploy/stacks/st' })
    expect(stackSubject(stack({ status: 'drifted', drift: [{ kind: 'image_changed', service: 'w', field: 'image', desired: 'a', observed: 'b', since_ms: 3 }] }))).toMatchObject({ level: 'warning', sinceMs: 3 })
    expect(stackSubject(stack({ status: 'running' }))?.level).toBe('ok')
    expect(stackSubject(stack({ status: 'pending' }))?.level).toBe('ok')
    expect(stackSubject(stack({ status: 'gone' }))).toBeNull()
    expect(stackSubject(stack({ status: 'failed', site: 'divtik' }))?.sites).toEqual(['divtik'])
  })

  it('a rollout paused on failures is Critical, at the gate Warning, finished absent', () => {
    expect(rolloutSubject(rollout({ state: 'paused_threshold' }))).toMatchObject({ kind: 'rollout', level: 'critical', sites: ['divtik'], dest: '/deploy/rollouts/ro' })
    expect(rolloutSubject(rollout({ state: 'paused_gate' }))?.level).toBe('warning')
    expect(rolloutSubject(rollout({ state: 'running' }))?.level).toBe('ok')
    expect(rolloutSubject(rollout({ state: 'completed' }))).toBeNull()
    expect(rolloutSubject(rollout({ state: 'draft' }))).toBeNull()
  })

  it('the Deploy world sits between Infra and Ops, only when asked for', () => {
    expect(boardWorlds().map((w) => w.key)).toEqual(['frontend', 'backend', 'infra', 'ops'])
    expect(boardWorlds({ deploy: true }).map((w) => w.key)).toEqual(['frontend', 'backend', 'infra', 'deploy', 'ops'])
    expect(boardWorlds({ deploy: true, ops: false, sites: true }).map((w) => w.key)).toEqual(['frontend', 'backend', 'infra', 'deploy', 'sites'])
  })
})
