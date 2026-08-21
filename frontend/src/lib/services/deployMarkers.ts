// Releases → chart markers.
//
// One place, because a deploy line has to look and read the same on every chart it appears on. A
// marker whose label says `v1.4.2` on the latency chart and `1.4.2` on the volume chart beside it
// costs the reader a moment working out whether they are the same release.
import type { Deployment } from '@/lib/core/api'

/** BaseChart's marker contract, shared by `LineChart` and `BarChart`. */
export interface ChartMarker {
  xMs: number
  label: string
  color: string
}

/**
 * How many markers a chart can carry before they stop being readable. A service that ships forty
 * times in the window would otherwise turn the plot into a picket fence, which hides the very
 * thing the markers exist to reveal.
 */
export const MAX_MARKERS = 12

const COLOR = 'hsl(var(--brand))'

/**
 * Deploy markers for a chart, newest first, capped at `max`.
 *
 * The cap keeps the NEWEST releases rather than a spread across the window: when you are looking at
 * a chart with a step in it, the release you are hunting is almost always a recent one, and an
 * evenly-sampled subset would be as likely to drop it as to keep it.
 */
export function deployMarkers(deployments: Deployment[], max: number = MAX_MARKERS): ChartMarker[] {
  return [...deployments]
    .sort((a, b) => b.detected_at_ms - a.detected_at_ms)
    .slice(0, Math.max(0, max))
    .map((d) => ({ xMs: d.detected_at_ms, label: d.version, color: COLOR }))
}
