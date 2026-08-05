import { it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { createRouter, createMemoryHistory } from 'vue-router'
import { TooltipProvider } from '@/components/ui/tooltip'
import AppShell from './AppShell.vue'
import ContextBar from './ContextBar.vue'

vi.mock('@/lib/core/api', () => ({
  api: { mock: false, tenants: vi.fn().mockResolvedValue({ tenants: [] }) },
}))

const router = createRouter({
  history: createMemoryHistory(),
  routes: [{ path: '/:x(.*)*', component: { template: '<div/>' } }],
})

// Note: this asserts on the ContextBar *component* rather than just page text, because
// NavRail already renders a visible "Logs" nav-item label unconditionally — a bare
// `w.text()).toContain('Logs')` check would pass even without ContextBar mounted, defeating
// the RED step of TDD. Asserting the component + its crumb prop is the meaningful check.
it('AppShell renders the global ContextBar with the crumb', async () => {
  await router.push('/logs')
  await router.isReady()
  const w = mount(
    {
      components: { AppShell, TooltipProvider },
      template:
        '<TooltipProvider><AppShell crumb="Logs"><div class="child"/></AppShell></TooltipProvider>',
    },
    {
      global: {
        plugins: [
          router,
          [VueQueryPlugin, { queryClient: new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } }) }],
        ],
      },
      attachTo: document.body,
    },
  )
  const bar = w.findComponent(ContextBar)
  expect(bar.exists()).toBe(true)
  expect(bar.props('crumb')).toBe('Logs')
  expect(w.find('.child').exists()).toBe(true)
})
