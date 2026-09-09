import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import VolumesTable from './VolumesTable.vue'
import VolumeRemoveDialog from './VolumeRemoveDialog.vue'
import type { VolumeInfo } from '@/lib/core/api'

const vol = (name: string, used_by: string[]): VolumeInfo => ({ name, driver: 'local', mountpoint: `/var/lib/docker/volumes/${name}/_data`, size_bytes: 2 ** 30, used_by, created_ms: 0 })
const stubs = { Tooltip: { template: '<div><slot /></div>' }, TooltipTrigger: { template: '<div><slot /></div>' }, TooltipContent: { template: '<div><slot /></div>' } }

describe('VolumesTable', () => {
  it('offers remove only for a volume nothing mounts, and emits the row', async () => {
    const w = mount(VolumesTable, { props: { volumes: [vol('pgdata', ['lenz-db-1']), vol('orphan', [])] }, global: { stubs } })
    const buttons = w.findAll('[data-volume-action="remove"]')
    expect(buttons).toHaveLength(2)
    expect(buttons[0].attributes('disabled')).toBeDefined()
    expect(buttons[1].attributes('disabled')).toBeUndefined()
    await buttons[1].trigger('click')
    expect(w.emitted('remove')?.[0]?.[0]).toMatchObject({ name: 'orphan' })
  })

  it('disables every remove while the role or the agent blocks lifecycle actions', () => {
    const w = mount(VolumesTable, { props: { volumes: [vol('orphan', [])], blocked: 'Requires the Operator role' }, global: { stubs } })
    expect(w.get('[data-volume-action="remove"]').attributes('disabled')).toBeDefined()
  })
})

describe('VolumeRemoveDialog', () => {
  it('wants the name typed before it will submit, and says the size', async () => {
    const w = mount(VolumeRemoveDialog, { props: { open: true, volume: vol('orphan', []), hostStatus: 'online' }, attachTo: document.body })
    await w.vm.$nextTick()
    const submit = () => document.querySelector('[data-testid="volume-remove-submit"]') as HTMLButtonElement
    expect(document.body.querySelector('[data-testid="volume-remove"]')?.textContent).toContain('1.0 GB')
    expect(submit().disabled).toBe(true)
    const input = document.querySelector('[data-testid="volume-typed"]') as HTMLInputElement
    input.value = 'orpha'
    input.dispatchEvent(new Event('input'))
    await w.vm.$nextTick()
    expect(submit().disabled).toBe(true)
    input.value = 'orphan'
    input.dispatchEvent(new Event('input'))
    await w.vm.$nextTick()
    expect(submit().disabled).toBe(false)
    submit().click()
    expect(w.emitted('confirm')).toHaveLength(1)
    w.unmount()
  })
})
