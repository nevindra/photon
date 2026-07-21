// One host in the /infra fleet card grid: name + worst-resource degraded flag, CPU/MEM/DSK/GPU
// meter rows (a null resource skips its row), and a relative last-seen footer. Real rendered
// behavior (mount + read text/DOM), not mocks — mirrors MonitorCard.test.js's card-component style.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HostCard from './HostCard.vue'
import type { InfraHost } from '@/lib/core/api'

function makeHost(overrides: Partial<InfraHost> = {}): InfraHost {
  return {
    host: 'web-1',
    cpuUtil: 0.32,
    memUtil: 0.54,
    diskUtil: 0.45,
    gpuUtil: null,
    lastSeenNs: (BigInt(Date.now() - 12_000) * 1_000_000n).toString(),
    hasGpu: false,
    ...overrides,
  }
}

describe('HostCard', () => {
  it('renders the host name, meters and percents from a fixture', () => {
    const w = mount(HostCard, { props: { host: makeHost() } })
    expect(w.text()).toContain('web-1')
    expect(w.text()).toContain('CPU')
    expect(w.text()).toContain('32%')
    expect(w.text()).toContain('MEM')
    expect(w.text()).toContain('54%')
    expect(w.text()).toContain('DSK')
    expect(w.text()).toContain('45%')
    expect(w.text()).toMatch(/ago/)
  })

  it('skips the DSK row when diskUtil is null', () => {
    const w = mount(HostCard, { props: { host: makeHost({ diskUtil: null }) } })
    expect(w.text()).not.toContain('DSK')
  })

  it('renders a GPU row only when gpuUtil is present', () => {
    const withoutGpu = mount(HostCard, { props: { host: makeHost() } })
    expect(withoutGpu.text()).not.toContain('GPU')

    const withGpu = mount(HostCard, { props: { host: makeHost({ hasGpu: true, gpuUtil: 0.61 }) } })
    expect(withGpu.text()).toContain('GPU')
    expect(withGpu.text()).toContain('61%')
  })

  it('emits select with the host name on click', async () => {
    const w = mount(HostCard, { props: { host: makeHost() } })
    await w.trigger('click')
    expect(w.emitted('select')?.[0]).toEqual(['web-1'])
  })

  it('shows a worst-resource flag and border tint when degraded', () => {
    const degraded = mount(HostCard, { props: { host: makeHost({ cpuUtil: 0.95, memUtil: 0.5 }) } })
    // Scope to the flag testid, not w.text() — the CPU meter row's own label also contains "CPU",
    // so a whole-card text assertion can't tell the flag apart from that row.
    const flag = degraded.find('[data-testid="host-card-flag"]')
    expect(flag.exists()).toBe(true)
    expect(flag.text()).toBe('⚠ CPU')
    expect(degraded.html()).toMatch(/sev-error/)

    const warning = mount(HostCard, { props: { host: makeHost({ diskUtil: 0.85 }) } })
    expect(warning.find('[data-testid="host-card-flag"]').text()).toBe('⚠ DSK')
    expect(warning.html()).toMatch(/sev-warn/)
    expect(warning.html()).not.toMatch(/sev-error/)

    const healthy = mount(HostCard, { props: { host: makeHost() } })
    expect(healthy.find('[data-testid="host-card-flag"]').exists()).toBe(false)
    expect(healthy.html()).not.toMatch(/sev-error|sev-warn/)
  })
})
