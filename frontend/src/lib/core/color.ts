// Small colour helpers for the canvas charts. uPlot paints fills/strokes from plain colour STRINGS
// on a <canvas>, where — unlike stacked DOM divs — a translucent fill composites over whatever bar
// is already painted behind it, not over the page background. A stacked bar draws each segment from
// the zero baseline (largest cumulative behind, smaller in front), so a front segment tinted with
// alpha over the opaque segment behind it barely changes colour and its band vanishes. The fix is to
// pre-flatten such tints into OPAQUE colours against the surface they visually sit on (the card bg).

// A parsed HSL channel triplet, in the same units shadcn-style CSS variables use: hue in degrees,
// saturation/lightness in percent (0..100).
export interface HslChannels {
  h: number
  s: number
  l: number
}

// An {r,g,b} colour with each channel in 0..255.
export interface Rgb {
  r: number
  g: number
  b: number
}

// Parse a shadcn-style HSL channel triplet ("H S% L%", e.g. "0 0% 45.1%") into {h,s,l} numbers.
// Returns null if the string isn't a plain triplet (so callers can fall back gracefully).
export function parseHslTriplet(triplet: unknown): HslChannels | null {
  const m = String(triplet ?? '')
    .trim()
    .match(/^(-?[\d.]+)\s+([\d.]+)%\s+([\d.]+)%$/)
  if (!m) return null
  return { h: Number(m[1]), s: Number(m[2]), l: Number(m[3]) }
}

// HSL (h in degrees, s/l in percent) → {r,g,b} 0..255. The compact CSS-Color-4 formulation; for a
// greyscale colour (s = 0) this collapses to r = g = b = round(255 · l/100).
export function hslToRgb(h: number, s: number, l: number): Rgb {
  const sn = s / 100
  const ln = l / 100
  const k = (n: number) => (n + h / 30) % 12
  const a = sn * Math.min(ln, 1 - ln)
  const f = (n: number) => ln - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1))
  return {
    r: Math.round(255 * f(0)),
    g: Math.round(255 * f(8)),
    b: Math.round(255 * f(4)),
  }
}

// Composite an HSL-triplet foreground over an HSL-triplet background at `alpha` (0..1) and return an
// OPAQUE `rgb(r, g, b)` string — the colour a translucent `hsl(fg / alpha)` fill would show when laid
// on a solid `bg`. Use this for stacked-bar segment fills so each band reads distinctly on the canvas
// (see the module note). Falls back to a plain `hsl(<fg>)` if either triplet can't be parsed.
export function flattenHsl(fgTriplet: unknown, bgTriplet: unknown, alpha: number): string {
  const fg = parseHslTriplet(fgTriplet)
  const bg = parseHslTriplet(bgTriplet)
  if (!fg || !bg) return `hsl(${fgTriplet})`
  const a = Math.max(0, Math.min(1, alpha))
  const f = hslToRgb(fg.h, fg.s, fg.l)
  const b = hslToRgb(bg.h, bg.s, bg.l)
  const mix = (x: number, y: number) => Math.round(x * a + y * (1 - a))
  return `rgb(${mix(f.r, b.r)}, ${mix(f.g, b.g)}, ${mix(f.b, b.b)})`
}
