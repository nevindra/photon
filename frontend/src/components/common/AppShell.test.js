import { describe, it, expect, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { defineComponent, h } from 'vue'
import { createRouter, createMemoryHistory } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import NavRail from '@/components/common/NavRail.vue'
import { TooltipProvider } from '@/components/ui/tooltip'
import { authed } from '@/lib/core/auth'

// AppShell now owns nav + logout via the router (auth.logout hits api.logout).
vi.mock('@/lib/core/api', () => ({
  api: { mock: false, logout: vi.fn().mockResolvedValue(undefined) },
}))

const routes = [
  { path: '/logs', component: { template: '<div />' } },
  { path: '/traces', component: { template: '<div />' } },
  { path: '/traces/:traceId', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]

async function mountShell(initial = '/logs') {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(initial)
  await router.isReady()
  // AppShell renders NavRail (Reka Tooltip) → needs a TooltipProvider ancestor.
  const Harness = defineComponent({
    setup: () => () => h(TooltipProvider, null, { default: () => h(AppShell) }),
  })
  const wrapper = mount(Harness, { global: { plugins: [router] }, attachTo: document.body })
  return { wrapper, router }
}

describe('AppShell (router nav)', () => {
  it('pushes the selected section route on NavRail select', async () => {
    const { wrapper, router } = await mountShell('/logs')
    wrapper.findComponent(NavRail).vm.$emit('select', 'traces')
    await flushPromises()
    expect(router.currentRoute.value.path).toBe('/traces')
    wrapper.unmount()
  })

  it('derives the active section from the current route', async () => {
    const { wrapper } = await mountShell('/traces/abc123')
    expect(wrapper.findComponent(NavRail).props('active')).toBe('traces')
    wrapper.unmount()
  })

  it('logs out and routes to /login on NavRail logout', async () => {
    const { api } = await import('@/lib/core/api')
    authed.value = true
    const { wrapper, router } = await mountShell('/logs')
    wrapper.findComponent(NavRail).vm.$emit('logout')
    await flushPromises()
    expect(api.logout).toHaveBeenCalled()
    expect(authed.value).toBe(false)
    expect(router.currentRoute.value.path).toBe('/login')
    wrapper.unmount()
  })
})
