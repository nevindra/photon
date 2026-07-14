// Namespace grouping + search ranking for the metric picker. Pure, no Vue, no storage.
export interface MetricEntry {
  name: string
  type?: string
  unit?: string
  is_monotonic?: boolean | null
  series_count?: number
}
export interface NamespaceGroup {
  name: string // namespace prefix; '' is the trailing "Other" bucket (singletons / no-separator)
  metrics: MetricEntry[]
}

const byName = (a: MetricEntry, b: MetricEntry): number => a.name.localeCompare(b.name)

// First path segment before a '.' or '_' (OTEL uses '.', Prometheus remote-write uses '_'); null if none.
export function namespaceOf(name: string): string | null {
  const m = /^([A-Za-z0-9]+)[._]/.exec(name)
  return m ? m[1] : null
}

export function groupByNamespace(entries: MetricEntry[]): NamespaceGroup[] {
  const groups = new Map<string, MetricEntry[]>()
  for (const e of entries) {
    const ns = namespaceOf(e.name) ?? ''
    const list = groups.get(ns) ?? []
    list.push(e)
    groups.set(ns, list)
  }
  const other: MetricEntry[] = [...(groups.get('') ?? [])]
  const named: NamespaceGroup[] = []
  for (const [ns, metrics] of groups) {
    if (ns === '') continue
    if (metrics.length === 1) other.push(...metrics) // a lone metric isn't worth a header
    else named.push({ name: ns, metrics: [...metrics].sort(byName) })
  }
  named.sort((a, b) => a.name.localeCompare(b.name))
  if (other.length) named.push({ name: '', metrics: other.sort(byName) })
  return named
}

// Case-insensitive substring filter, ranked: prefix (0) < word-boundary after a separator (1) <
// substring (2). Non-matches dropped; ties broken by name.
export function rankMetrics(entries: MetricEntry[], query: string): MetricEntry[] {
  const q = query.trim().toLowerCase()
  if (!q) return [...entries].sort(byName)
  const scored: { e: MetricEntry; score: number }[] = []
  for (const e of entries) {
    const n = e.name.toLowerCase()
    const idx = n.indexOf(q)
    if (idx < 0) continue
    const score = idx === 0 ? 0 : /[._]/.test(n[idx - 1] ?? '') ? 1 : 2
    scored.push({ e, score })
  }
  return scored.sort((a, b) => a.score - b.score || byName(a.e, b.e)).map((s) => s.e)
}
