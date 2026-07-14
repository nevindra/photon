import { defineComponent } from 'vue'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { useChartTheme } from './useChartTheme.js'

// tokens.css triplets keyed by theme — the mocked getComputedStyle reads the live `.dark` class
// so re-resolving after a class toggle yields genuinely different colours (not just a version bump).
const TRIPLETS = {
  light: { '--foreground': '0 0% 3.9%', '--muted-foreground': '0 0% 45.1%', '--border': '0 0% 89.8%', '--popover': '0 0% 100%' },
  dark: { '--foreground': '0 0% 98%', '--muted-foreground': '0 0% 63.9%', '--border': '0 0% 14.9%', '--popover': '0 0% 3.9%' },
}

// Install a getComputedStyle whose values depend on <html>.class. `empty:true` simulates jsdom
// returning '' for unresolved custom props (exercises the FALLBACK path).
function stubComputedStyle({ empty = false } = {}) {
  vi.stubGlobal('getComputedStyle', () => ({
    getPropertyValue: (name) => {
      if (empty) return ''
      const set = document.documentElement.classList.contains('dark') ? TRIPLETS.dark : TRIPLETS.light
      return set[name] ?? ''
    },
  }))
}

// A trivial host so the composable's onMounted/onUnmounted hooks run.
const Harness = defineComponent({
  setup: () => useChartTheme(),
  render: () => null,
})

// MutationObserver callbacks are microtasks in jsdom — flush with a macrotask turn.
const flush = () => new Promise((r) => setTimeout(r, 0))

beforeEach(() => {
  document.documentElement.classList.remove('dark')
})
afterEach(() => {
  vi.unstubAllGlobals()
  document.documentElement.classList.remove('dark')
})

describe('useChartTheme', () => {
  it('resolves the light-theme tokens into concrete colour strings', () => {
    stubComputedStyle()
    const wrapper = mount(Harness)
    const t = wrapper.vm.theme

    expect(t.text).toBe('hsl(0 0% 3.9%)') // --foreground
    expect(t.muted).toBe('hsl(0 0% 45.1%)') // --muted-foreground
    expect(t.axis).toBe('hsl(0 0% 45.1%)')
    expect(t.border).toBe('hsl(0 0% 89.8%)') // --border
    expect(t.tooltipBg).toBe('hsl(0 0% 100%)') // --popover
    expect(t.tooltipBorder).toBe('hsl(0 0% 89.8%)')
    // faint inks carry a low alpha built from the raw triplet
    expect(t.grid).toBe('hsl(0 0% 3.9% / 0.07)')
    expect(t.crosshair).toBe('hsl(0 0% 3.9% / 0.35)')

    wrapper.unmount()
  })

  it('re-resolves and bumps version when the .dark class toggles on <html>', async () => {
    stubComputedStyle()
    const wrapper = mount(Harness)
    expect(wrapper.vm.theme.text).toBe('hsl(0 0% 3.9%)')
    const before = wrapper.vm.version

    document.documentElement.classList.add('dark')
    await flush()

    expect(wrapper.vm.version).toBeGreaterThan(before)
    expect(wrapper.vm.theme.text).toBe('hsl(0 0% 98%)') // dark --foreground
    expect(wrapper.vm.theme.tooltipBg).toBe('hsl(0 0% 3.9%)') // dark --popover
    expect(wrapper.vm.theme.grid).toBe('hsl(0 0% 98% / 0.07)')

    wrapper.unmount()
  })

  it('falls back to light defaults when tokens resolve empty (jsdom-safe)', () => {
    stubComputedStyle({ empty: true })
    const wrapper = mount(Harness)
    expect(wrapper.vm.theme.text).toBe('hsl(0 0% 3.9%)')
    expect(wrapper.vm.theme.border).toBe('hsl(0 0% 89.8%)')
    wrapper.unmount()
  })

  it('disconnects the observer on unmount (no bump after teardown)', async () => {
    stubComputedStyle()
    const wrapper = mount(Harness)
    const theme = wrapper.vm.theme
    const version = wrapper.vm.version
    wrapper.unmount()

    document.documentElement.classList.add('dark')
    await flush()

    // refs captured pre-unmount are unchanged: the observer is gone.
    expect(version).toBe(wrapper.vm.version)
    expect(theme.text).toBe('hsl(0 0% 3.9%)')
  })
})
