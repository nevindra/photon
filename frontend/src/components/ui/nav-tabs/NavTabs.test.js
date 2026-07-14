// NavTabs/NavTabItem are route-based section sub-navigation: real <RouterLink>s under
// a <nav>, not the ARIA tablist widget. RouterLink needs router context to resolve `to`
// into an href, so we mount against a real minimal router (mirrors the harness idiom in
// MetricsExplorer.test.js) rather than stubbing it out.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { createRouter, createWebHistory } from 'vue-router'
import { NavTabs, NavTabItem } from './index.js'

function makeHarness() {
  const router = createRouter({
    history: createWebHistory(),
    routes: [
      { path: '/a', name: 'a', component: { template: '<div/>' } },
      { path: '/b', name: 'b', component: { template: '<div/>' } },
    ],
  })
  return router
}

function mountWrapped(router) {
  return mount(
    {
      components: { NavTabs, NavTabItem },
      template: `
        <NavTabs aria-label="Section">
          <NavTabItem to="/a" :active="true" data-testid="tab-a">A</NavTabItem>
          <NavTabItem to="/b" :active="false">B</NavTabItem>
        </NavTabs>
      `,
    },
    { global: { plugins: [router] } },
  )
}

describe('NavTabs / NavTabItem', () => {
  it('renders both items as anchors with the correct hrefs', async () => {
    const router = makeHarness()
    router.push('/a')
    await router.isReady()
    const wrapper = mountWrapped(router)

    const anchors = wrapper.findAll('a')
    expect(anchors.length).toBe(2)
    expect(anchors[0].attributes('href')).toBe('/a')
    expect(anchors[1].attributes('href')).toBe('/b')
  })

  it('marks the active item with aria-current and data-state=active', async () => {
    const router = makeHarness()
    router.push('/a')
    await router.isReady()
    const wrapper = mountWrapped(router)

    const active = wrapper.get('[data-testid="tab-a"]')
    expect(active.attributes('aria-current')).toBe('page')
    expect(active.attributes('data-state')).toBe('active')
  })

  it('leaves the inactive item without aria-current, with data-state=inactive', async () => {
    const router = makeHarness()
    router.push('/a')
    await router.isReady()
    const wrapper = mountWrapped(router)

    const anchors = wrapper.findAll('a')
    const inactive = anchors[1]
    expect(inactive.attributes('aria-current')).toBeUndefined()
    expect(inactive.attributes('data-state')).toBe('inactive')
  })

  it('falls through a consumer-passed data-testid to the rendered anchor', async () => {
    const router = makeHarness()
    router.push('/a')
    await router.isReady()
    const wrapper = mountWrapped(router)

    const anchor = wrapper.get('[data-testid="tab-a"]')
    expect(anchor.element.tagName).toBe('A')
  })
})
