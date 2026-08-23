// `tokens.css` → concrete colours a chart engine can paint with.
//
// Every chart in Photon is styled from the design tokens, but a canvas or an SVG `fill` cannot take
// `hsl(var(--sev-error))` — the variable has to be resolved against the document first. uPlot's
// side of this already exists (`useChartTheme.js`); this is the same job for TanStack Charts, kept
// separate so the two engines can coexist without either one owning the other's colour rules.
//
// Resolution is deliberately live rather than a build-time constant: the app has a light/dark
// toggle, and a chart whose colours were frozen at import time renders yesterday's theme after a
// flip. `useChartTheme`'s `version` ref is the signal that a flip happened.
import { computed, type ComputedRef } from 'vue'
// The theme watcher is plain JS; it exports `{ version }`, a ref bumped on a light↔dark flip.
import { useChartTheme } from '@/components/charts/useChartTheme.js'

/** The channels a chart needs from the token system. */
export interface TanstackChartTheme {
  foreground: string
  muted: string
  border: string
  grid: string
  brand: string
  ok: string
  warn: string
  error: string
  /** Emerald. There is no `--sev-good` token — see `ApdexBandChart`'s note. */
  good: string
}

/**
 * Read one CSS custom property as an `hsl()` colour, falling back to a literal when the document is
 * absent (SSR, unit tests) or the token is not defined.
 *
 * The fallbacks are not decoration: a chart that renders `hsl()` — an invalid colour — draws
 * nothing at all, so every lookup has to land somewhere real.
 */
function hslToken(style: CSSStyleDeclaration | null, name: string, fallback: string): string {
  const raw = (style?.getPropertyValue(name) || '').trim()
  return `hsl(${raw || fallback})`
}

export function useTanstackChartTheme(): ComputedRef<TanstackChartTheme> {
  const { version } = useChartTheme() as { version: { value: number } }
  return computed<TanstackChartTheme>(() => {
    // Touching `version` is what re-runs this on a light↔dark flip; the value itself is unused.
    void version.value
    const style =
      typeof document !== 'undefined' && typeof getComputedStyle === 'function'
        ? getComputedStyle(document.documentElement)
        : null
    return {
      foreground: hslToken(style, '--foreground', '0 0% 9%'),
      muted: hslToken(style, '--muted-foreground', '0 0% 45.1%'),
      border: hslToken(style, '--border', '0 0% 89.8%'),
      // The gridlines sit behind the data, so they take the border colour at low opacity rather
      // than a lighter token — one line that stays correct in both themes.
      grid: hslToken(style, '--border', '0 0% 89.8%'),
      brand: hslToken(style, '--brand', '187 100% 42%'),
      ok: hslToken(style, '--muted-foreground', '0 0% 45.1%'),
      warn: hslToken(style, '--sev-warn', '38 92% 50%'),
      error: hslToken(style, '--sev-error', '0 72% 51%'),
      good: 'hsl(160 84% 39%)',
    }
  })
}
