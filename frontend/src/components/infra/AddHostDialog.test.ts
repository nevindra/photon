// The Add-host dialog: validation before anything leaves the browser, the SSH path starting a
// bootstrap job and rendering its stepper, and the manual path showing a minted token ONCE with
// an install snippet. The role gate is exercised through auth's `role` ref.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { role } from '@/lib/core/auth'
import AddHostDialog from './AddHostDialog.vue'

const startBootstrap = vi.fn()
const bootstrapJob = vi.fn()
const enrollAgent = vi.fn()
vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    tenants: vi.fn().mockResolvedValue({ tenants: [] }),
    startBootstrap: (...a: unknown[]) => startBootstrap(...a),
    bootstrapJob: (...a: unknown[]) => bootstrapJob(...a),
    enrollAgent: (...a: unknown[]) => enrollAgent(...a),
  },
}))

function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } })
  return [VueQueryPlugin, { queryClient }]
}

async function mountDialog(props: Record<string, unknown> = {}) {
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }] })
  router.push('/')
  await router.isReady()
  const wrapper = mount(
    {
      components: { TooltipProvider, AddHostDialog },
      props: ['open', 'prefillHost'],
      template: '<TooltipProvider><AddHostDialog :open="open" :prefill-host="prefillHost" /></TooltipProvider>',
    },
    { props: { open: true, prefillHost: null, ...props }, global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  await flushPromises()
  return wrapper
}

const q = (sel: string) => document.body.querySelector(sel) as HTMLElement | null

describe('AddHostDialog', () => {
  beforeEach(() => {
    role.value = 'admin'
    startBootstrap.mockReset()
    bootstrapJob.mockReset()
    enrollAgent.mockReset()
    document.body.innerHTML = ''
  })

  it('refuses to start an SSH install without an address or password, before calling the api', async () => {
    const wrapper = await mountDialog()
    ;(q('[data-testid="add-host-submit"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(q('[data-testid="add-host-error"]')?.textContent).toContain('address')
    expect(startBootstrap).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('starts a bootstrap job and renders one stepper per host', async () => {
    const job = {
      id: 'bj_1',
      status: 'running',
      started_ms: Date.now(),
      credentials_kept: false,
      hosts: [
        { address: 'edge-1', host: null, status: 'running', error: null, steps: [{ key: 'connect', label: 'Connect over SSH', status: 'done' }, { key: 'detect', label: 'Detect OS and architecture', status: 'running' }] },
        { address: 'edge-2', host: null, status: 'failed', error: 'refused', steps: [{ key: 'connect', label: 'Connect over SSH', status: 'failed', detail: 'refused' }] },
      ],
    }
    startBootstrap.mockResolvedValue({ ok: true, job })
    bootstrapJob.mockResolvedValue(job)
    const wrapper = await mountDialog()
    const addr = q('[data-testid="ssh-addresses"]') as HTMLTextAreaElement
    addr.value = 'edge-1\nedge-2'
    addr.dispatchEvent(new Event('input'))
    const pw = q('[data-testid="ssh-password"]') as HTMLInputElement
    pw.value = 'secret'
    pw.dispatchEvent(new Event('input'))
    await flushPromises()
    expect(q('[data-testid="add-host-submit"]')?.textContent).toContain('Install on 2 hosts')
    ;(q('[data-testid="add-host-submit"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(startBootstrap).toHaveBeenCalledTimes(1)
    const input = startBootstrap.mock.calls[0][0]
    expect(input.addresses).toEqual(['edge-1', 'edge-2'])
    expect(input.remember_credentials).toBe(false)
    expect(input.capabilities).toContain('read')
    const progress = q('[data-testid="bootstrap-progress"]')
    expect(progress).not.toBeNull()
    expect(progress!.querySelectorAll('[data-step="connect"]')).toHaveLength(2)
    expect(progress!.querySelector('[data-step="connect"][data-status="failed"]')?.textContent).toContain('refused')
    wrapper.unmount()
  })

  it('manual path mints a token and shows it once with the install snippet', async () => {
    enrollAgent.mockResolvedValue({ ok: true, token: 'ak_minted', host: { host: 'edge-9' } })
    const wrapper = await mountDialog({ prefillHost: 'edge-9' })
    expect((q('[data-testid="manual-host"]') as HTMLInputElement).value).toBe('edge-9')
    ;(q('[data-testid="add-host-submit"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(enrollAgent).toHaveBeenCalledWith(expect.objectContaining({ host: 'edge-9' }))
    const panel = q('[data-testid="agent-token-panel"]')
    expect(panel).not.toBeNull()
    expect(panel!.textContent).toContain('ak_minted')
    expect(panel!.textContent).toContain('/api/agent/download')
    wrapper.unmount()
  })

  it('disables the submit for a role below admin and says why', async () => {
    role.value = 'operator'
    const wrapper = await mountDialog()
    const gated = q('[data-perm="agents"][data-gated="true"]')
    expect(gated).not.toBeNull()
    expect(gated!.querySelector('button')?.hasAttribute('disabled')).toBe(true)
    wrapper.unmount()
  })
})
