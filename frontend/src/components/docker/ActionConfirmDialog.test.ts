import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ActionConfirmDialog from './ActionConfirmDialog.vue'
import type { Container } from '@/lib/core/api'

const container: Container = {
  id: 'c1', name: 'api', image: 'x:1', image_id: 'sha256:a', command: '', state: 'running', health: 'none', status_text: '', created_ms: 0, started_ms: 0, finished_ms: null, exit_code: null,
  restart_count: 0, restart_policy: 'no', ports: [{ host_ip: null, host_port: 8080, container_port: 80, protocol: 'tcp' }], mounts: [], networks: [], labels: {}, env_keys: [], env: {},
  compose_project: null, compose_service: null, managed_stack_id: null, cpu_pct: null, mem_bytes: null, mem_limit_bytes: null, net_rx_bps: null, net_tx_bps: null, block_read_bps: null, block_write_bps: null,
}

const q = (sel: string) => document.body.querySelector(sel) as HTMLElement | null

describe('ActionConfirmDialog', () => {
  it('a restart asks once and explains the gap; an offline host says it queues', async () => {
    const wrapper = mount(ActionConfirmDialog, { props: { open: true, kind: 'restart', container: { ...container }, hostStatus: 'offline' }, attachTo: document.body })
    await wrapper.vm.$nextTick()
    expect(q('[data-testid="action-confirm"]')?.textContent).toContain('8080')
    expect(q('[data-testid="action-offline-note"]')).not.toBeNull()
    expect(q('[data-testid="action-submit"]')?.textContent).toContain('Queue restart')
    ;(q('[data-testid="action-submit"]') as HTMLButtonElement).click()
    expect(wrapper.emitted('confirm')?.[0]).toEqual([{}])
    wrapper.unmount()
  })

  it('a remove wants the name typed and carries the volumes choice', async () => {
    const wrapper = mount(ActionConfirmDialog, { props: { open: true, kind: 'remove', container: { ...container }, hostStatus: 'online' }, attachTo: document.body })
    await wrapper.vm.$nextTick()
    const submit = q('[data-testid="action-submit"]') as HTMLButtonElement
    expect(submit.disabled).toBe(true)
    const typed = q('[data-testid="action-typed"]') as HTMLInputElement
    typed.value = 'api'
    typed.dispatchEvent(new Event('input'))
    await wrapper.vm.$nextTick()
    expect(submit.disabled).toBe(false)
    submit.click()
    expect(wrapper.emitted('confirm')?.[0]).toEqual([{ remove_volumes: false }])
    wrapper.unmount()
  })
})
