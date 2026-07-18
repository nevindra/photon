import { describe, it, expect, vi, afterEach } from 'vitest'
import { defineComponent } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import { api, type AlertRuleInput, type AlertRuleResult } from '@/lib/core/api'
import {
  alertRulesQueryKey,
  alertChannelsQueryKey,
  alertIncidentsQueryKey,
  useRules,
  useCreateRule,
} from '@/lib/alertsQueries'

afterEach(() => vi.restoreAllMocks())

// Mirrors `dataQueries.test.js`'s `mountHarness`: `useQuery`/`useMutation` need an active Vue
// injection context, so every composable under test is exercised inside a mounted component.
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

describe('alertsQueries', () => {
  it('query keys are stable and parameterized', () => {
    expect(alertRulesQueryKey()).toEqual(['alerts', 'rules'])
    expect(alertChannelsQueryKey()).toEqual(['alerts', 'channels'])
    expect(alertIncidentsQueryKey({ status: 'triggered', rule_id: 'rule-1', limit: 50 })).toEqual([
      'alerts',
      'incidents',
      'triggered',
      'rule-1',
      50,
    ])
    // Unfiltered call still yields a stable key (empty-string placeholders, not undefined).
    expect(alertIncidentsQueryKey({})).toEqual(['alerts', 'incidents', '', '', ''])
  })

  it('useRules() builds the ["alerts","rules"] query key and calls api.alertRules', async () => {
    const spy = vi.spyOn(api, 'alertRules').mockResolvedValue([])
    const { queryClient } = mountHarness(() => useRules())
    await flushPromises()

    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ signal: expect.anything() }))
    const keys = queryClient.getQueryCache().getAll().map((q) => q.queryKey)
    expect(keys).toContainEqual(['alerts', 'rules'])
  })

  it('useCreateRule() posts the exact rule body shape to api.createAlertRule and does not throw on failure', async () => {
    const spy = vi.spyOn(api, 'createAlertRule').mockResolvedValue({ ok: true } as AlertRuleResult)
    const { query } = mountHarness(() => useCreateRule())

    const body: AlertRuleInput = {
      name: 'High CPU (web fleet)',
      description: 'Sustained CPU pressure across web hosts',
      enabled: true,
      signal: 'metrics',
      condition: {
        signal: 'metrics',
        metric_name: 'system.cpu.utilization',
        group_by: ['host.name'],
        agg: 'avg',
        window_secs: 300,
        cmp: 'gt',
        threshold: 0.9,
      },
      for_secs: 300,
      interval_secs: 60,
      severity: 'warning',
      channel_ids: ['chan-1'],
    }
    query.mutate(body)
    await flushPromises()

    expect(spy).toHaveBeenCalledWith(body)
    expect(query.isError.value).toBe(false)

    // A validation failure resolves as `{ ok: false }`, it never rejects the mutation.
    spy.mockResolvedValueOnce({ ok: false, error: 'name must not be empty' })
    query.mutate(body)
    await flushPromises()
    expect(query.isError.value).toBe(false)
  })
})
