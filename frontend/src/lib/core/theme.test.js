import { describe, it, expect, beforeEach } from 'vitest'
import { useTheme } from '@/lib/core/theme'

describe('theme', () => {
  beforeEach(() => {
    window.localStorage.clear()
  })

  it('setTheme persists the choice to localStorage', () => {
    const { setTheme } = useTheme()

    setTheme('dark')
    expect(window.localStorage.getItem('photon-theme')).toBe('dark')

    setTheme('light')
    expect(window.localStorage.getItem('photon-theme')).toBe('light')
  })

  it('setTheme toggles the .dark class on documentElement', () => {
    const { setTheme } = useTheme()

    setTheme('dark')
    expect(document.documentElement.classList.contains('dark')).toBe(true)

    setTheme('light')
    expect(document.documentElement.classList.contains('dark')).toBe(false)
  })

  it('toggle() flips between light and dark, updating state, storage and DOM', () => {
    const { theme, isDark, toggle, setTheme } = useTheme()

    setTheme('light')
    expect(theme.value).toBe('light')
    expect(isDark.value).toBe(false)

    toggle()
    expect(theme.value).toBe('dark')
    expect(isDark.value).toBe(true)
    expect(window.localStorage.getItem('photon-theme')).toBe('dark')
    expect(document.documentElement.classList.contains('dark')).toBe(true)

    toggle()
    expect(theme.value).toBe('light')
    expect(isDark.value).toBe(false)
    expect(document.documentElement.classList.contains('dark')).toBe(false)
  })
})
