import { computed, ref, type Ref, type ComputedRef } from 'vue'

const STORAGE_KEY = 'photon-theme'

type Theme = 'light' | 'dark'

/**
 * Resolve the initial theme: stored preference wins, else the OS/browser
 * `prefers-color-scheme`, else 'light'. SSR-safe (guards `window`).
 * @returns {'light'|'dark'}
 */
function resolveInitialTheme(): Theme {
  if (typeof window === 'undefined') return 'light'

  const stored = window.localStorage?.getItem(STORAGE_KEY)
  if (stored === 'light' || stored === 'dark') return stored

  if (window.matchMedia?.('(prefers-color-scheme: dark)').matches) return 'dark'

  return 'light'
}

function applyTheme(t: Theme): void {
  if (typeof document === 'undefined') return
  document.documentElement.classList.toggle('dark', t === 'dark')
}

// Module-level singleton so every `useTheme()` call shares the same state.
const theme: Ref<Theme> = ref(resolveInitialTheme())
applyTheme(theme.value)

const isDark: ComputedRef<boolean> = computed(() => theme.value === 'dark')

/**
 * Persist `t` to localStorage and apply it to `document.documentElement`.
 * @param {'light'|'dark'} t
 */
function setTheme(t: Theme): void {
  theme.value = t
  if (typeof window !== 'undefined') {
    window.localStorage?.setItem(STORAGE_KEY, t)
  }
  applyTheme(t)
}

function toggle(): void {
  setTheme(theme.value === 'dark' ? 'light' : 'dark')
}

/**
 * Shared theme state: `{ theme, isDark, toggle, setTheme }`.
 */
export function useTheme(): {
  theme: Ref<Theme>
  isDark: ComputedRef<boolean>
  toggle: () => void
  setTheme: (t: Theme) => void
} {
  return { theme, isDark, toggle, setTheme }
}

/**
 * Apply the resolved initial theme immediately (e.g. from `main.js`) to avoid
 * a flash of the wrong theme before any component mounts.
 */
export function initTheme(): void {
  applyTheme(theme.value)
}
