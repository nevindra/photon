import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import RolloutStageBoard from './RolloutStageBoard.vue'
import type { Rollout } from '@/lib/core/api'

const rollout: Rollout = {
  id: 'r', name: 'r', template_id: 't', template_name: 'T', template_version_id: 'tv', template_version: '2', previous_version: '1', stack_name: 't',
  selector: { kind: 'hosts', hosts: ['a', 'b', 'c'] }, strategy: { canary: 1, canary_is_percent: false, failure_threshold: 1, prepull: true, auto_promote: false, health_grace_secs: 300 },
  state: 'paused_gate', stage: 1, stages: 2,
  targets: [
    { host: 'a', site: null, stage: 1, status: 'healthy', attempts: 1, last_error: null, stack_id: 'st', previous_version: '1', agent_status: 'online', updated_ms: 0 },
    { host: 'b', site: null, stage: 2, status: 'pending', attempts: 0, last_error: null, stack_id: null, previous_version: '1', agent_status: 'offline', updated_ms: 0 },
    { host: 'c', site: null, stage: 0, status: 'skipped', attempts: 0, last_error: 'agent installed without --docker-apply', stack_id: null, previous_version: null, agent_status: 'online', updated_ms: 0 },
  ],
  variables: {}, created_by: 'r', created_ms: 0, updated_ms: 0, started_ms: 0, finished_ms: null,
}

describe('RolloutStageBoard', () => {
  it('draws the canary, the locked gate, the rest, and the skipped panel', async () => {
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }] })
    router.push('/')
    await router.isReady()
    const wrapper = mount(RolloutStageBoard, { props: { rollout }, global: { plugins: [router] } })
    const stages = wrapper.findAll('[data-stage]')
    expect(stages).toHaveLength(2)
    expect(stages[0].text()).toContain('Canary')
    expect(stages[0].find('[data-host="a"]').text()).toContain('1 → 2')
    expect(wrapper.find('[data-testid="stage-gate"]').text()).toContain('Promote to continue')
    expect(stages[1].find('[data-host="b"]').text()).toContain('offline')
    expect(wrapper.find('[data-testid="skipped-targets"]').text()).toContain('--docker-apply')
    wrapper.unmount()
  })
})
