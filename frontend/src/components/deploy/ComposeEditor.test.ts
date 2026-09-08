// The editor's checks panel: services and unresolved variables update as the text changes, and a
// risky construct blocks `ready` until it is acknowledged.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ComposeEditor from './ComposeEditor.vue'

const SAFE = 'services:\n  app:\n    image: x:${VERSION}\n'
const RISKY = 'services:\n  app:\n    image: x:1\n    privileged: true\n'

describe('ComposeEditor', () => {
  it('reports services, unset variables, and readiness', async () => {
    const wrapper = mount(ComposeEditor, { props: { files: [{ path: 'compose.yaml', content: SAFE }, { path: '.env', content: '' }], acknowledged: [] } })
    expect(wrapper.find('[data-check="services"]').text()).toContain('1 service')
    expect(wrapper.find('[data-check="variables"]').text()).toContain('VERSION')
    expect((wrapper.vm as any).ready).toBe(true)
    await wrapper.setProps({ files: [{ path: 'compose.yaml', content: SAFE }, { path: '.env', content: 'VERSION=2\n' }] })
    expect(wrapper.find('[data-check="variables"]').text()).toContain('Every variable resolves')
    wrapper.unmount()
  })

  it('is not ready while a risky construct is unacknowledged', async () => {
    const wrapper = mount(ComposeEditor, { props: { files: [{ path: 'compose.yaml', content: RISKY }], acknowledged: [] } })
    expect(wrapper.find('[data-testid="compose-warnings"]').exists()).toBe(true)
    expect(wrapper.find('[data-warning="privileged:app"]').exists()).toBe(true)
    expect((wrapper.vm as any).ready).toBe(false)
    await wrapper.setProps({ acknowledged: ['privileged:app'] })
    expect((wrapper.vm as any).ready).toBe(true)
    wrapper.unmount()
  })

  it('edits the active file and refuses unsafe new paths', async () => {
    const wrapper = mount(ComposeEditor, { props: { files: [{ path: 'compose.yaml', content: SAFE }], acknowledged: [] } })
    await wrapper.find('[data-testid="file-content"]').setValue('services:\n  a:\n    image: x\n  b:\n    image: y\n')
    const emitted = wrapper.emitted('update:files')!
    expect((emitted[0][0] as { content: string }[])[0].content).toContain('b:')
    await wrapper.find('[data-testid="add-file"]').trigger('click')
    await wrapper.find('[data-testid="new-file-path"]').setValue('../evil')
    await wrapper.find('form').trigger('submit')
    expect(wrapper.text()).toContain('inside the stack folder')
    wrapper.unmount()
  })
})
