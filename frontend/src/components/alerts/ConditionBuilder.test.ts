import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { QueryClient, VueQueryPlugin } from '@tanstack/vue-query'
import type { AlertCondition } from '@/lib/core/api'
import ConditionBuilder from './ConditionBuilder.vue'

// The builder fires autocomplete queries on mount; a bare QueryClient is enough (they stay pending).
function mountBuilder(condition: unknown) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return mount(ConditionBuilder, {
    props: { condition: condition as AlertCondition | null },
    global: { plugins: [[VueQueryPlugin, { queryClient: qc }]] },
  })
}

describe('ConditionBuilder label_filters round-trip', () => {
  it('re-emits a seeded host.name label filter', async () => {
    const seeded = {
      signal: 'metrics',
      metric_name: 'system.cpu.utilization',
      label_filters: { 'host.name': 'web-01' },
      agg: 'avg',
      window_secs: 300,
      cmp: 'gt',
      threshold: 0.9,
    }
    const wrapper = mountBuilder(seeded)
    const emitted = wrapper.emitted('update:condition') as unknown[][] | undefined
    expect(emitted, 'should emit at least once (immediate)').toBeTruthy()
    const last = emitted!.at(-1)![0] as { label_filters?: Record<string, string> }
    expect(last.label_filters).toEqual({ 'host.name': 'web-01' })
  })
})
