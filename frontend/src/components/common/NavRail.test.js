import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, h } from 'vue'
import NavRail from '@/components/common/NavRail.vue'
import { TooltipProvider } from '@/components/ui/tooltip'

// NavRail uses Reka Tooltip, which requires an ancestor TooltipProvider (see
// App.vue). Wrap it here the same way, and mount to document.body since
// Tooltip/DropdownMenu content is teleported (portalled) out of the component
// subtree — jsdom querying needs to look at document.body, not the wrapper.
function mountNavRail(props = {}) {
  const Harness = defineComponent({
    props: { mock: { type: Boolean, default: false } },
    emits: ['logout', 'select'],
    setup(harnessProps, { emit }) {
      return () =>
        h(TooltipProvider, null, {
          default: () =>
            h(NavRail, {
              mock: harnessProps.mock,
              onLogout: () => emit('logout'),
              onSelect: (key) => emit('select', key),
            }),
        })
    },
  })
  return mount(Harness, { props, attachTo: document.body })
}

async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0))
}

describe('NavRail — avatar dropdown', () => {
  it('exposes a Sign out item and emits logout when activated', async () => {
    const wrapper = mountNavRail()

    const trigger = wrapper.find('[aria-label="Account menu"]')
    expect(trigger.exists()).toBe(true)
    await trigger.trigger('click')
    await flush()

    const signOutItem = Array.from(document.body.querySelectorAll('[role="menuitem"]')).find(
      (el) => el.textContent.includes('Sign out'),
    )
    expect(signOutItem).toBeTruthy()

    signOutItem.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }))
    signOutItem.dispatchEvent(new MouseEvent('pointerup', { bubbles: true }))
    signOutItem.click()
    await flush()

    expect(wrapper.emitted('logout')).toBeTruthy()
    expect(wrapper.emitted('logout').length).toBe(1)

    wrapper.unmount()
  })
})

describe('NavRail — connected indicator', () => {
  it('is green and labeled "connected" when mock is false', () => {
    const wrapper = mountNavRail({ mock: false })
    const dot = wrapper.find('[data-testid="connected-dot"]')
    expect(dot.exists()).toBe(true)
    expect(dot.classes()).toContain('bg-green-500')
    expect(dot.classes()).not.toContain('bg-amber-500')

    const label = wrapper.find('[aria-label="connected"]')
    expect(label.exists()).toBe(true)

    wrapper.unmount()
  })

  it('is amber and labeled "demo data" when mock is true', () => {
    const wrapper = mountNavRail({ mock: true })
    const dot = wrapper.find('[data-testid="connected-dot"]')
    expect(dot.exists()).toBe(true)
    expect(dot.classes()).toContain('bg-amber-500')
    expect(dot.classes()).not.toContain('bg-green-500')

    const label = wrapper.find('[aria-label="demo data"]')
    expect(label.exists()).toBe(true)

    wrapper.unmount()
  })
})

describe('NavRail — grouped worlds', () => {
  // Task 10 regroup: Home + Frontend/Backend/Ops worlds + Explore (Logs/Traces/
  // Metrics) + Manage (Data), each item stamped with data-nav="item.key" for lookup/E2E hooks.
  function wrap(active) {
    return mount(
      {
        components: { NavRail, TooltipProvider },
        template: `<TooltipProvider><NavRail active="${active}" /></TooltipProvider>`,
      },
      { attachTo: document.body },
    )
  }

  it('renders Home + the three worlds + Explore tools + Manage', () => {
    const t = wrap('home').text()
    for (const label of [
      'Home',
      'Frontend',
      'Backend',
      'Ops',
      'Logs',
      'Traces',
      'Metrics',
      'Data',
    ]) {
      expect(t).toContain(label)
    }
  })

  it('marks the active group', () => {
    const w = wrap('backend')
    expect(w.get('[data-nav="backend"]').classes().join(' ')).toContain('text-brand')
  })
})

describe('NavRail — navigation', () => {
  it('enables the Traces tab and emits select("traces")', async () => {
    const wrapper = mountNavRail()
    const tracesBtn = wrapper.findAll('button').find((b) => b.text().includes('Traces'))
    expect(tracesBtn).toBeTruthy()
    await tracesBtn.trigger('click')
    expect(wrapper.emitted('select')).toBeTruthy()
    expect(wrapper.emitted('select')[0]).toEqual(['traces'])
    wrapper.unmount()
  })

  it('renders a Data nav item that emits select("data")', async () => {
    const w = mountNavRail()
    const btn = w.findAll('button').find((b) => b.text().includes('Data'))
    expect(btn).toBeTruthy()
    await btn.trigger('click')
    expect(w.emitted('select').some(([k]) => k === 'data')).toBe(true)
    w.unmount()
  })
})
