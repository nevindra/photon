// Muted-but-distinct categorical palette for service identity in the waterfall + service map
// ONLY (the rest of the app stays black & white). Errors are always red and override this.
// Class strings are LITERALS so Tailwind's content scanner keeps them (never build dynamically).
export const SERVICE_PALETTE = [
  'bg-sky-500/70',
  'bg-violet-500/70',
  'bg-emerald-500/70',
  'bg-amber-500/70',
  'bg-rose-500/70',
  'bg-cyan-500/70',
  'bg-indigo-500/70',
  'bg-teal-500/70',
  'bg-fuchsia-500/70',
  'bg-lime-500/70',
] as const

// Stable hash → palette index, so a service keeps its colour across renders.
export function serviceColorClass(name: string | undefined): string {
  const s = name ?? ''
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0
  return SERVICE_PALETTE[h % SERVICE_PALETTE.length]
}
