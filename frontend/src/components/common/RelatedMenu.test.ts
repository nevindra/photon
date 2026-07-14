import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import RelatedMenu from './RelatedMenu.vue'
import { scope, timeRange, customRange } from '@/lib/core/context'

// Reka DropdownMenu portals (teleports) its content to document.body, so once the menu is open we
// query the document rather than the component wrapper (mirrors NavRail.test.js's account menu).
const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:x(.*)*', component: { template: '<div/>' } }],
})

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

describe('RelatedMenu', () => {
  it('navigates to a related destination carrying context', async () => {
    timeRange.value = '15m'
    customRange.value = null
    scope.value = null
    // Memory history performs no initial navigation until pushed/installed — push first so
    // router.isReady() resolves (mirrors NavRail/ServiceDetailView tests) instead of deadlocking.
    router.push('/')
    await router.isReady()

    const w = mount(RelatedMenu, {
      props: { entity: { kind: 'span', fields: { traceId: 't1', spanId: 's1', service: 'checkout' } } },
      global: { plugins: [router] },
      attachTo: document.body,
    })

    await w.get('[data-testid="related-trigger"]').trigger('click')
    await flush()

    const item = document.querySelector<HTMLElement>('[data-related-id="logs-span"]')
    expect(item).toBeTruthy()

    item!.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }))
    item!.dispatchEvent(new MouseEvent('pointerup', { bubbles: true }))
    item!.click()
    await flush()

    const fullPath = router.currentRoute.value.fullPath
    // Context-carrying URL: the span→logs term AND the active time window rode along.
    expect(fullPath).toContain('trace_id%3At1')
    expect(fullPath).toContain('range=15m')

    w.unmount()
  })
})
