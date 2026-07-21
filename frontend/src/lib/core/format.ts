// Severity model — ordered low→high. Each severity has a colour "tone".
//
// Colour policy (professional B&W theme): debug/info are NEUTRAL (no colour) so the
// coloured lines are the signal; only warn/error/fatal carry colour. `toneClasses`
// centralises the tone→Tailwind mapping so the level tag, filter-rail dot, row tick,
// and histogram all agree.

// The four colour "tones" a severity can carry. `neutral` = no colour (debug/info).
export type Tone = 'neutral' | 'warn' | 'error' | 'fatal'

export interface Severity {
  key: string
  label: string
  tone: Tone
}

export const SEVERITIES: Severity[] = [
  { key: 'debug', label: 'Debug', tone: 'neutral' },
  { key: 'info', label: 'Info', tone: 'neutral' },
  { key: 'warn', label: 'Warn', tone: 'warn' },
  { key: 'error', label: 'Error', tone: 'error' },
  { key: 'fatal', label: 'Fatal', tone: 'fatal' },
]

const BY_KEY: Record<string, Severity> = Object.fromEntries(SEVERITIES.map((s) => [s.key, s]))

export function severity(key: string): Severity {
  return BY_KEY[key] ?? BY_KEY.info
}

// tone → Tailwind class set. `text` for foreground (tag text / icon), `bgSoft` for a
// tinted background (tag / row), `solid` for a filled swatch (row tick / dot / bar).
// Full literal strings so Tailwind's content scanner keeps them (never build class
// names dynamically or they get purged).
export interface ToneClasses {
  text: string
  bgSoft: string
  solid: string
}

const TONE: Record<Tone, ToneClasses> = {
  neutral: { text: 'text-muted-foreground', bgSoft: 'bg-muted', solid: 'bg-muted-foreground' },
  warn: { text: 'text-sev-warn', bgSoft: 'bg-sev-warn-soft', solid: 'bg-sev-warn' },
  error: { text: 'text-sev-error', bgSoft: 'bg-sev-error-soft', solid: 'bg-sev-error' },
  fatal: { text: 'text-sev-fatal', bgSoft: 'bg-sev-fatal-soft', solid: 'bg-sev-fatal' },
}

// `tone` accepts any string (unknown tones fall back to `neutral`) — internal
// lookup cast is safe because of that fallback.
export function toneClasses(tone: string): ToneClasses {
  return TONE[tone as Tone] ?? TONE.neutral
}

// Convenience: severity key → its tone class set.
export function severityClasses(key: string): ToneClasses {
  return toneClasses(severity(key).tone)
}

const pad = (n: number, w = 2): string => String(n).padStart(w, '0')

// Epoch nanoseconds (BigInt) → "14:22:07.104"
export function formatClock(nanos: bigint): string {
  const d = new Date(Number(nanos / 1_000_000n))
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(
    d.getMilliseconds(),
    3,
  )}`
}

// Epoch nanoseconds (BigInt) → "Jul 1 · 14:22:07.104"
export function formatFull(nanos: bigint): string {
  const d = new Date(Number(nanos / 1_000_000n))
  const month = d.toLocaleString('en-US', { month: 'short' })
  return `${month} ${d.getDate()} · ${formatClock(nanos)}`
}

export function formatNumber(n: number): string {
  return Number(n).toLocaleString('en-US')
}

// Large counts → compact label, e.g. 128_400 → "128.4k", 2_300_000 → "2.3M", 940 → "940".
export function formatCompact(n: number): string {
  const v = Number(n)
  if (!Number.isFinite(v)) return '—'
  const abs = Math.abs(v)
  const trim = (x: number): string => x.toFixed(1).replace(/\.0$/, '')
  if (abs < 1_000) return String(Math.round(v))
  if (abs < 1_000_000) return trim(v / 1_000) + 'k'
  if (abs < 1_000_000_000) return trim(v / 1_000_000) + 'M'
  return trim(v / 1_000_000_000) + 'B'
}

// Bytes → human-readable string using binary units, e.g. 210_000_000 → "200.3 MB".
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null) return '—'
  const n = Number(bytes)
  if (!Number.isFinite(n)) return '—'
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB', 'PB']
  let val = n / 1024
  let i = 0
  while (val >= 1024 && i < units.length - 1) {
    val /= 1024
    i++
  }
  return `${val.toFixed(1)} ${units[i]}`
}

// Bytes/second → compact rate label, e.g. 2_150_000 → "2.1 MB/s", 512 → "512 B/s".
export function formatRate(bytesPerSec: number | null | undefined): string {
  const label = formatBytes(bytesPerSec)
  return label === '—' ? label : `${label}/s`
}

// Compact relative label, e.g. "12s ago", "4m ago".
export function relative(nanos: bigint, nowMs: number = Date.now()): string {
  const secs = Math.max(0, Math.round((nowMs - Number(nanos / 1_000_000n)) / 1000))
  if (secs < 60) return `${secs}s ago`
  const mins = Math.round(secs / 60)
  if (mins < 60) return `${mins}m ago`
  return `${Math.round(mins / 60)}h ago`
}

// Nanoseconds (Number or BigInt) → compact duration label: ns / µs / ms / s.
export function formatDuration(ns: number | bigint | null | undefined): string {
  if (ns == null) return '—'
  const n = Number(ns)
  if (n < 1_000) return `${n}ns`
  if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}µs`
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}ms`
  return `${(n / 1_000_000_000).toFixed(2)}s`
}
