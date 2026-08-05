import { describe, it, expect, vi, afterEach } from 'vitest'
import { defineComponent } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { api, type TenantSummary } from '@/lib/core/api'
import { tenantsQueryKey, tenantsSummaryQueryKey, useTenants, useTenantsSummary } from '@/lib/tenants/tenantsQueries'

afterEach(() => vi.restoreAllMocks())

// Mirrors `alertsQueries.test.ts`'s `mountHarness`: `useQuery` needs an active Vue injection
// context, so every composable under test is exercised inside a mounted component.
function mountHarness<T>(setupFn: () => T) {
  const Harness = defineComponent({
    setup() {
      return { result: setupFn() }
    },
    render: () => null,
  })
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const wrapper = mount(Harness, { global: { plugins: [[VueQueryPlugin, { queryClient }]] } })
  return { wrapper, queryClient, query: wrapper.vm.result as T }
}

describe('tenantsQueries', () => {
  it('query keys are stable', () => {
    expect(tenantsQueryKey()).toEqual(['tenants'])
    expect(tenantsSummaryQueryKey()).toEqual(['tenants', 'summary'])
  })

  it('useTenants() builds the ["tenants"] query key and calls api.tenants', async () => {
    const spy = vi.spyOn(api, 'tenants').mockResolvedValue({ tenants: [] })
    const { queryClient } = mountHarness(() => useTenants())
    await flushPromises()

    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ signal: expect.anything() }))
    const keys = queryClient.getQueryCache().getAll().map((q) => q.queryKey)
    expect(keys).toContainEqual(['tenants'])
  })

  it('useTenantsSummary() calls api.tenantsSummary and resolves the mocked payload', async () => {
    const summary: TenantSummary[] = [
      {
        name: 'divtik',
        mode: 'summary',
        status: 'up',
        last_seen_ms: 1_700_000_000_000,
        ingest_rows_per_sec: 42,
        open_incidents: 0,
        hot_bytes: 1024,
        ui_url: 'https://divtik.example.com',
        spark: [[1_700_000_000_000, 42]],
      },
    ]
    const spy = vi.spyOn(api, 'tenantsSummary').mockResolvedValue(summary)
    const { query } = mountHarness(() => useTenantsSummary())
    await flushPromises()

    expect(spy).toHaveBeenCalled()
    expect(query.data.value).toEqual(summary)
  })
})
