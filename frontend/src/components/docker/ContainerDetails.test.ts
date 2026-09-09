import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import ContainerDetails from './ContainerDetails.vue'
import type { Container } from '@/lib/core/api'

const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:x(.*)*', component: { template: '<div/>' } }] })

function fixture(): Container {
  return {
    id: '23ad0445510e76685f3d6c704e999819a735043919eb5bb5504eece5c07ad625',
    name: 'lenz-lenz-1',
    image: 'registry.gitlab.com/nodefluxio/lenz:latest',
    image_id: 'sha256:3738bda9d671',
    command: './entrypoint.sh',
    state: 'running',
    health: 'none',
    status_text: 'Up 3 hours',
    created_ms: 0,
    started_ms: 0,
    finished_ms: null,
    exit_code: 0,
    restart_count: 0,
    restart_policy: 'unless-stopped',
    ports: [{ container_port: 80, host_port: null, protocol: 'tcp', host_ip: null }],
    mounts: [{ type: 'bind', source: '/tmp/scratchpad/maps', destination: '/var/maps', rw: true }],
    networks: ['lenz_default'],
    labels: { 'io.photon.stack': 'stk_1', 'com.docker.compose.project': 'lenz' },
    env_keys: ['VISIONAIRE_HOST', 'DB_PASSWORD', 'DB_HOST'],
    env: { VISIONAIRE_HOST: '192.168.103.76:4004', DB_PASSWORD: '••••••••', DB_HOST: 'db' },
    compose_project: 'lenz',
    compose_service: 'lenz',
    managed_stack_id: 'stk_1',
    cpu_pct: null, mem_bytes: null, mem_limit_bytes: null, net_rx_bps: null, net_tx_bps: null, block_read_bps: null, block_write_bps: null,
  } as unknown as Container
}

function mountDetails() {
  return mount(ContainerDetails, { props: { container: fixture() }, global: { plugins: [router] } })
}

describe('ContainerDetails — find in details', () => {
  it('shows every section idle, env sorted by name, secrets masked', () => {
    const w = mountDetails()
    for (const id of ['overview', 'ports', 'env', 'labels', 'mounts']) expect(w.find(`[data-testid="details-${id}"]`).exists()).toBe(true)
    const envKeys = w.find('[data-testid="details-env"]').findAll('dt').map((d) => d.text())
    expect(envKeys).toEqual(['DB_HOST', 'DB_PASSWORD', 'VISIONAIRE_HOST'])
    expect(w.find('[data-testid="details-env"]').findAll('[data-testid="masked"]').length).toBe(1)
    expect(w.text()).not.toContain('••••••••')
    expect(w.find('[data-testid="details-env"]').text()).toContain('1 masked')
    w.unmount()
  })

  it('filters every section to the rows containing the query and marks each hit', async () => {
    const w = mountDetails()
    await w.find('[data-testid="details-search"]').setValue('host')
    // Env: DB_HOST (key) + VISIONAIRE_HOST (key) survive, DB_PASSWORD does not.
    const env = w.find('[data-testid="details-env"]')
    expect(env.findAll('dt').map((d) => d.text())).toEqual(['DB_HOST', 'VISIONAIRE_HOST'])
    expect(env.findAll('mark').map((m) => m.text())).toEqual(['HOST', 'HOST'])
    // Sections with no hit fold away; the count reports the total.
    expect(w.find('[data-testid="details-ports"]').exists()).toBe(false)
    expect(w.find('[data-testid="details-labels"]').exists()).toBe(false)
    expect(w.find('[data-testid="details-mounts"]').exists()).toBe(false)
    expect(w.find('[data-testid="details-hits"]').text()).toBe('2 matches')
    w.unmount()
  })

  it('matches values too — a path in a mount, a value in a label — case-insensitively', async () => {
    const w = mountDetails()
    await w.find('[data-testid="details-search"]').setValue('/VAR/maps')
    expect(w.find('[data-testid="details-mounts"]').exists()).toBe(true)
    expect(w.find('[data-testid="details-mounts"]').find('mark').text()).toBe('/var/maps')
    expect(w.find('[data-testid="details-env"]').exists()).toBe(false)
    await w.find('[data-testid="details-search"]').setValue('stk_1')
    expect(w.find('[data-testid="details-labels"]').findAll('[data-testid="details-row"]').length).toBe(1)
    w.unmount()
  })

  it('never matches a masked value, and says so when nothing matches', async () => {
    const w = mountDetails()
    await w.find('[data-testid="details-search"]').setValue('••••')
    expect(w.text()).toContain('Nothing matches')
    await w.find('[data-testid="details-search"]').setValue('')
    expect(w.find('[data-testid="details-overview"]').exists()).toBe(true)
    w.unmount()
  })
})
