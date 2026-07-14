// Resolves the app's CSS design tokens (in styles/tokens.css) into concrete color strings the
// uPlot canvas can paint with — the canvas can't read CSS classes, so we snapshot the tokens
// via getComputedStyle on <html> and hand back plain `hsl(...)` strings. The tokens are HSL
// channel triplets (e.g. `0 0% 45.1%`), so a "solid" token becomes `hsl(<triplet>)` and the
// faint grid/crosshair inks become `hsl(<triplet> / <alpha>)`.
//
// Theme is REACTIVE: a MutationObserver on <html>'s `class` (light ↔ `.dark`, toggled by the
// ThemeToggle) re-resolves the tokens and bumps `version` — chart consumers watch `version` and
// redraw/rebuild so the canvas tracks the theme. jsdom-safe: with no document/getComputedStyle
// it returns baked light-theme defaults and never throws (mirrors TraceMinimap's no-canvas guard).
import { onMounted, onUnmounted, ref } from 'vue'

// Faint inks are the same token (--foreground) at a low alpha: hairline dashed gridlines and the
// hover crosshair. Kept low so the data — not the chrome — is the signal (token colour policy).
const GRID_ALPHA = 0.07
const CROSSHAIR_ALPHA = 0.35

// Fallback triplets == the light palette in tokens.css. Used when a token resolves empty
// (jsdom returns '' for custom props with no inline value) or document is unavailable entirely.
const FALLBACK = {
  '--foreground': '0 0% 3.9%',
  '--muted-foreground': '0 0% 45.1%',
  '--border': '0 0% 89.8%',
  '--popover': '0 0% 100%',
  '--brand': '191 94% 38%',
}

// Build the theme object from a `raw(tokenName) -> "h s% l%"` accessor. One builder so the live
// (getComputedStyle) path and the baked-default path can't drift.
function buildTheme(raw) {
  const solid = (name) => `hsl(${raw(name)})`
  const alpha = (name, a) => `hsl(${raw(name)} / ${a})`
  return {
    text: solid('--foreground'), //          legend / axis-adjacent text
    muted: solid('--muted-foreground'), //    tooltip time header, secondary text
    axis: solid('--muted-foreground'), //     axis tick labels + tick stroke
    border: solid('--border'), //             axis border line
    brand: solid('--brand'), //               Photon Cyan: default lone-series line/sparkline stroke
    grid: alpha('--foreground', GRID_ALPHA), //        dashed gridlines
    crosshair: alpha('--foreground', CROSSHAIR_ALPHA), // vertical cursor line
    tooltipBg: solid('--popover'), //         floating tooltip background
    tooltipBorder: solid('--border'), //      floating tooltip hairline
  }
}

// Baked light-theme colours: the SSR/jsdom answer, and the per-token fallback source.
const DEFAULT_THEME = Object.freeze(buildTheme((name) => FALLBACK[name]))

// Snapshot the live tokens off <html>. Empty tokens (and a missing document) degrade to FALLBACK.
function resolveTheme() {
  if (typeof document === 'undefined' || typeof getComputedStyle !== 'function') {
    return { ...DEFAULT_THEME }
  }
  const cs = getComputedStyle(document.documentElement)
  return buildTheme((name) => (cs.getPropertyValue(name) || '').trim() || FALLBACK[name])
}

/**
 * Reactive chart colours resolved from the CSS design tokens on <html>.
 * @returns {{ theme: import('vue').Ref<object>, version: import('vue').Ref<number> }}
 *   `theme`   — reactive object of concrete colour strings (see buildTheme keys).
 *   `version` — bumps on every light↔dark re-resolve; consumers watch it to redraw/rebuild.
 */
export function useChartTheme() {
  // Resolve once up front so `theme.value` is populated before first paint.
  const theme = ref(resolveTheme())
  const version = ref(0)
  let observer = null

  onMounted(() => {
    if (typeof MutationObserver === 'undefined' || typeof document === 'undefined') return
    // Re-resolve + bump whenever the theme class flips on <html>.
    observer = new MutationObserver(() => {
      theme.value = resolveTheme()
      version.value += 1
    })
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
  })

  onUnmounted(() => {
    observer?.disconnect()
    observer = null
  })

  return { theme, version }
}
