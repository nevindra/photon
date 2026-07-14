// Single visual identity per observability signal, reused across the /data cards, the
// composition bar, and any chart legend — so "logs" (say) is always the same hue and the same
// icon no matter where it's rendered. Color is NOT a separate palette: it's derived from the same
// stable hash as seriesColor.js, so a signal's hue always matches its chart series color.
import type { Component } from 'vue'
import { ScrollText, Waypoints, ChartSpline, Activity, Database } from 'lucide-vue-next'
import { seriesColor } from '@/lib/core/seriesColor'

export type SignalKey = 'logs' | 'traces' | 'metrics' | 'uptime'

const ICONS: Record<SignalKey, Component> = {
  logs: ScrollText,
  traces: Waypoints,
  metrics: ChartSpline,
  uptime: Activity,
}

const FALLBACK_ICON: Component = Database

function isSignalKey(key: string): key is SignalKey {
  return key === 'logs' || key === 'traces' || key === 'metrics' || key === 'uptime'
}

// The hex stroke color for this signal — identical to what seriesColor() picks for the same key,
// so a signal card/legend chip and its chart series are always the same hue.
export function signalColor(key: string): string {
  return seriesColor(key).stroke
}

// The lucide-vue-next icon component for this signal, with a generic fallback for unknown keys.
export function signalIcon(key: string): Component {
  return isSignalKey(key) ? ICONS[key] : FALLBACK_ICON
}

export function signalMeta(key: string): { color: string; icon: Component } {
  return { color: signalColor(key), icon: signalIcon(key) }
}
