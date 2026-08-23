// Route sub-nav. Two things worth pinning: the active tab survives a drill-in (a child route keeps
// its parent tab lit), and every link carries the query string forward — a tab that resets the time
// range is a tab that loses your place.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import ServiceSubNav from './ServiceSubNav.vue'

const routes = [
  { path: '/services/:service', component: { template: '<div />' } },
  { path: '/services/:service/operations', component: { template: '<div />' } },
  { path: '/services/:service/operations/:operation', component: { template: '<div />' } },
  { path: '/services/:service/errors', component: { template: '<div />' } },
  { path: '/services/:service/errors/:fingerprint', component: { template: '<div />' } },
  { path: '/services/:service/dependencies', component: { template: '<div />' } },
]

async function mountAt(path: string) {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push(path)
  await router.isReady()
  return mount(ServiceSubNav, { props: { service: 'checkout' }, global: { plugins: [router] } })
}

const activeTab = (w: ReturnType<typeof mount>) =>
  w.findAll('[data-tab]').find((n) => n.attributes('data-state') === 'active')?.attributes('data-tab')

describe('ServiceSubNav', () => {
  it('lights Overview on the bare service route', async () => {
    expect(activeTab(await mountAt('/services/checkout'))).toBe('overview')
  })

  it('lights Operations on the operations route', async () => {
    expect(activeTab(await mountAt('/services/checkout/operations'))).toBe('operations')
  })

  it('keeps Operations lit on a single operation — a child is still that section', async () => {
    expect(activeTab(await mountAt('/services/checkout/operations/GET%20%2Fa'))).toBe('operations')
  })

  it('lights Issues on the errors route and keeps it lit on one issue', async () => {
    expect(activeTab(await mountAt('/services/checkout/errors'))).toBe('issues')
    expect(activeTab(await mountAt('/services/checkout/errors/abc123'))).toBe('issues')
  })

  it('does not light Overview just because every path starts with the base', async () => {
    const w = await mountAt('/services/checkout/dependencies')
    expect(activeTab(w)).toBe('dependencies')
  })

  it('carries the query string onto every tab link', async () => {
    const w = await mountAt('/services/checkout?range=6h&tenant=jkt')
    const hrefs = w.findAll('a').map((a) => a.attributes('href'))
    expect(hrefs.every((h) => h?.includes('range=6h') && h?.includes('tenant=jkt'))).toBe(true)
  })

  it('percent-encodes a service name that needs it', async () => {
    const router = createRouter({ history: createMemoryHistory(), routes })
    router.push('/services/x')
    await router.isReady()
    const w = mount(ServiceSubNav, { props: { service: 'my api' }, global: { plugins: [router] } })
    expect(w.findAll('a')[0].attributes('href')).toContain('my%20api')
  })
})
