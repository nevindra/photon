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
    await w.get('[data-testid="tenants-manage-trigger"]').trigger('click')
    await new Promise((r) => setTimeout(r))

    const nameInput = document.body.querySelector('[data-testid="new-tenant-name"]') as HTMLInputElement
    nameInput.value = 'divtik'
    nameInput.dispatchEvent(new Event('input'))
    await flushPromises()

    const form = document.body.querySelector('[data-testid="new-tenant-form"]') as HTMLFormElement
    form.dispatchEvent(new Event('submit', { cancelable: true }))
    await flushPromises()

    expect(document.body.textContent).toContain('tk_tenant_mock_divtik')
    expect(document.body.textContent).toContain('[federation]')
    expect(document.body.textContent).toContain('mode = "summary"')
  })
})
