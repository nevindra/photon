// DataTenants (the /data?tab=tenants body): a table of registered federation tenants + the
// TenantManageDialog trigger. Mirrors RumAppsView.test.ts's mocked-api + QueryClient convention.
import { describe, it, expect, vi, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import DataTenants from './DataTenants.vue'

const state = { tenants: [] as any[] }

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    tenants: vi.fn(() => Promise.resolve({ tenants: state.tenants })),
    createTenant: vi.fn((name: string) => Promise.resolve({ ok: true, token: `tk_tenant_mock_${name}` })),
    updateTenant: vi.fn(() => Promise.resolve({ ok: true })),
    rotateTenantToken: vi.fn(() => Promise.resolve({ ok: true, token: 'tk_tenant_rotated' })),
    deleteTenant: vi.fn(() => Promise.resolve({ ok: true })),
  },
}))

function mountTenants() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(DataTenants, {
    attachTo: document.body,
    global: { plugins: [[VueQueryPlugin, { queryClient }]] },
  })
}

afterEach(() => {
  document.body.innerHTML = ''
  state.tenants = []
  vi.clearAllMocks()
})

describe('DataTenants', () => {
  it('shows an empty state and a call to register the first tenant when there are none', async () => {
    const w = mountTenants()
    await flushPromises()
    expect(w.text()).toContain('Register your first tenant')
  })

  it('creating a tenant surfaces the minted federation TOML snippet', async () => {
    const w = mountTenants()
    await flushPromises()
    await w.get('[data-testid="tenants-add-trigger"]').trigger('click')
    await new Promise((r) => setTimeout(r))

    const nameInput = document.body.querySelector('[data-testid="tenant-name"]') as HTMLInputElement
    nameInput.value = 'divtik'
    nameInput.dispatchEvent(new Event('input'))
    await flushPromises()

    const form = document.body.querySelector('[data-testid="tenant-form"]') as HTMLFormElement
    form.dispatchEvent(new Event('submit', { cancelable: true }))
    await flushPromises()

    // Default snippet format is env vars (field teams deploy with docker compose, not toml).
    expect(document.body.textContent).toContain('tk_tenant_mock_divtik')
    expect(document.body.textContent).toContain('PHOTON_FEDERATION_ENDPOINT=')
    expect(document.body.textContent).toContain('PHOTON_FEDERATION_MODE=summary')
    expect(document.body.textContent).not.toContain('[federation]')

    // The TOML toggle re-renders the snippet as a `[federation]` block.
    const fmt = Array.from(document.body.querySelectorAll('[data-testid="tenant-snippet-format"] button'))
    ;(fmt.find((b) => b.textContent?.trim() === 'TOML') as HTMLButtonElement).click()
    await flushPromises()
    expect(document.body.textContent).toContain('[federation]')
    expect(document.body.textContent).toContain(':4318')
    expect(document.body.textContent).toContain('mode = "summary"')

    // The mode picker re-renders the snippet for full mode.
    const segs = Array.from(document.body.querySelectorAll('[data-testid="tenant-mode"] button'))
    ;(segs.find((b) => b.textContent?.trim() === 'Full') as HTMLButtonElement).click()
    await flushPromises()
    expect(document.body.textContent).toContain('mode = "full"')

    // Traces-only emits full mode + a signals subset.
    ;(segs.find((b) => b.textContent?.includes('Traces only')) as HTMLButtonElement).click()
    await flushPromises()
    expect(document.body.textContent).toContain('signals = ["traces"]')

    // Env format follows the mode picker too (still traces-only here).
    ;(fmt.find((b) => b.textContent?.trim() === 'Env') as HTMLButtonElement).click()
    await flushPromises()
    expect(document.body.textContent).toContain('PHOTON_FEDERATION_TOKEN=tk_tenant_mock_divtik')
    expect(document.body.textContent).toContain('PHOTON_FEDERATION_MODE=full')
    expect(document.body.textContent).toContain('PHOTON_FEDERATION_SIGNALS=traces')
    ;(fmt.find((b) => b.textContent?.trim() === 'TOML') as HTMLButtonElement).click()
    await flushPromises()

    // Copy buttons on the minted panel put the secret / snippet on the clipboard.
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.assign(navigator, { clipboard: { writeText } })
    ;(document.body.querySelector('[data-testid="tenant-copy-token"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith('tk_tenant_mock_divtik')
    ;(document.body.querySelector('[data-testid="tenant-copy-snippet"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(writeText).toHaveBeenLastCalledWith(expect.stringContaining('[federation]'))
  })

  it('row actions: edit opens the dialog for that tenant, delete fires the mutation', async () => {
    state.tenants = [{ name: 'divtik', token: '…abcd', ui_url: null, created_at: 0 }]
    const w = mountTenants()
    await flushPromises()

    await w.get('[data-testid="tenant-edit-divtik"]').trigger('click')
    await new Promise((r) => setTimeout(r))
    expect(document.body.textContent).toContain('Edit divtik')
    // Edit mode: no name field (the name is immutable), but the rotate action is present.
    expect(document.body.querySelector('[data-testid="tenant-name"]')).toBeNull()
    expect(document.body.querySelector('[data-testid="tenant-rotate"]')).not.toBeNull()
  })

  it('delete row action asks for confirmation before firing the mutation', async () => {
    const { api } = await import('@/lib/core/api')
    state.tenants = [{ name: 'divtik', token: '…abcd', ui_url: null, created_at: 0 }]
    const w = mountTenants()
    await flushPromises()

    await w.get('[data-testid="tenant-delete-divtik"]').trigger('click')
    await new Promise((r) => setTimeout(r))
    expect(api.deleteTenant).not.toHaveBeenCalled()
    expect(document.body.textContent).toContain('Delete divtik?')

    ;(document.body.querySelector('[data-testid="tenant-delete-confirm"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(api.deleteTenant).toHaveBeenCalledWith('divtik')
  })

  it('cancelling the delete confirmation fires nothing', async () => {
    const { api } = await import('@/lib/core/api')
    state.tenants = [{ name: 'divtik', token: '…abcd', ui_url: null, created_at: 0 }]
    const w = mountTenants()
    await flushPromises()

    await w.get('[data-testid="tenant-delete-divtik"]').trigger('click')
    await new Promise((r) => setTimeout(r))
    ;(document.body.querySelector('[data-testid="tenant-delete-cancel"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(api.deleteTenant).not.toHaveBeenCalled()
    expect(document.body.textContent).not.toContain('Delete divtik?')
  })

  it('rotate inside the edit dialog surfaces the rotated token snippet', async () => {
    state.tenants = [{ name: 'divtik', token: '…abcd', ui_url: null, created_at: 0 }]
    const w = mountTenants()
    await flushPromises()

    await w.get('[data-testid="tenant-edit-divtik"]').trigger('click')
    await new Promise((r) => setTimeout(r))
    ;(document.body.querySelector('[data-testid="tenant-rotate"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(document.body.textContent).toContain('tk_tenant_rotated')
    expect(document.body.textContent).toContain('PHOTON_FEDERATION_TOKEN=tk_tenant_rotated')
  })
})
