// The Sources dialog's Image registries tab: the credentials hosts pull private images with —
// listed without their token, stored from the form, rotated in place, removed.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import RegistriesDialog from './RegistriesDialog.vue'

const credential = {
  id: 'rc_1', server: 'registry.gitlab.com', username: 'nodeflux-deploy', label: 'GitLab deploy token', scope: { kind: 'site', sites: ['jakarta'] },
  created_ms: Date.now() - 60_000, created_by: 'rina', updated_ms: Date.now() - 60_000,
}
const api = vi.hoisted(() => ({
  mock: false,
  registries: vi.fn(),
  imageRegistries: vi.fn(),
  addImageRegistry: vi.fn(),
  updateImageRegistry: vi.fn(),
  deleteImageRegistry: vi.fn(),
}))
vi.mock('@/lib/core/api', () => ({ api }))
vi.mock('@/components/ui/toast', () => ({ toast: vi.fn() }))

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  return [VueQueryPlugin, { queryClient: new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } }) }]
}
async function mountDialog() {
  const wrapper = mount(
    { components: { TooltipProvider, RegistriesDialog }, template: '<TooltipProvider><RegistriesDialog :open="true" /></TooltipProvider>' },
    { global: { plugins: [queryPlugin()] }, attachTo: document.body },
  )
  await flushPromises()
  const tab = document.querySelector('[data-testid="mode-credentials"]') as HTMLElement
  tab.click()
  await flushPromises()
  return wrapper
}
function setInput(id: string, value: string): void {
  const el = document.getElementById(id) as HTMLInputElement
  el.value = value
  el.dispatchEvent(new Event('input', { bubbles: true }))
}

describe('RegistriesDialog · Image registries', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    vi.clearAllMocks()
    api.registries.mockResolvedValue({ registries: [] })
    api.imageRegistries.mockResolvedValue({ credentials: [credential] })
    api.addImageRegistry.mockResolvedValue({ ok: true, credential: { ...credential, id: 'rc_2', server: 'ghcr.io', username: 'bot' } })
    api.updateImageRegistry.mockResolvedValue({ ok: true, credential })
    api.deleteImageRegistry.mockResolvedValue({ ok: true })
  })

  it('lists credentials with their scope and never a token, and stores a new one from the form', async () => {
    const wrapper = await mountDialog()
    const row = document.querySelector('[data-credential="rc_1"]') as HTMLElement
    expect(row.textContent).toContain('registry.gitlab.com')
    expect(row.textContent).toContain('nodeflux-deploy')
    expect(row.textContent).toContain('site jakarta')
    expect(document.body.textContent).not.toContain('glpat')

    const submit = document.querySelector('[data-testid="credential-submit"]') as HTMLButtonElement
    expect(submit.disabled).toBe(true)
    setInput('cred-server', 'https://ghcr.io/')
    setInput('cred-user', 'bot')
    setInput('cred-secret', 'ghp_xyz')
    await flushPromises()
    expect(submit.disabled).toBe(false)
    ;(document.querySelector('[data-testid="credential-form"]') as HTMLFormElement).dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
    await flushPromises()
    expect(api.addImageRegistry).toHaveBeenCalledWith({ server: 'https://ghcr.io/', username: 'bot', secret: 'ghp_xyz', label: null, scope: null })
    // The token field is cleared after a store.
    expect((document.getElementById('cred-secret') as HTMLInputElement).value).toBe('')
    wrapper.unmount()
  })

  it('rotates a token in place and removes a credential', async () => {
    const wrapper = await mountDialog()
    const rotate = Array.from(document.querySelectorAll('[data-credential="rc_1"] button')).find((b) => b.textContent?.includes('Rotate token')) as HTMLButtonElement
    rotate.click()
    await flushPromises()
    setInput('rotate-rc_1', 'glpat-new')
    await flushPromises()
    const form = document.querySelector('[data-credential="rc_1"] form') as HTMLFormElement
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
    await flushPromises()
    expect(api.updateImageRegistry).toHaveBeenCalledWith('rc_1', { secret: 'glpat-new' })

    const remove = document.querySelector('[data-credential="rc_1"] [aria-label="Remove credential for registry.gitlab.com"]') as HTMLButtonElement
    remove.click()
    await flushPromises()
    expect(api.deleteImageRegistry).toHaveBeenCalledWith('rc_1')
    wrapper.unmount()
  })
})
