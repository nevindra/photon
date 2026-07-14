// Pure trace-assembly + geometry helpers for the waterfall. Time values are BigInt nanoseconds.
// Defensive by design (spec error-handling): orphans become roots, cycles can't infinite-loop,
// clock skew is flagged and offsets are clamped to >= 0.

// Raw nanosecond timestamp/duration fields as they arrive off the wire: already BigInt in most
// call sites, but occasionally a string/number (e.g. JSON-decoded) — toBig() normalizes all of
// these to `bigint` without precision loss.
type Nanos = bigint | number | string | null | undefined

// Minimal shape isSpanError actually needs (duck-typed so callers can pass a bare
// `{ status_code }` without satisfying the full TraceSpanInput shape).
export interface SpanErrorLike {
  status_code?: number | null
}

// Shape of a single span as consumed by buildTrace/getTraceTree. Only the fields the tree
// assembly logic reads are required; everything else (trace_id, attributes, events, links,
// status_text, status_message, ...) rides along on `span` for consumers (SpanDetailPanel,
// TraceWaterfall, TracePeekDrawer) but isn't touched here, so it's modeled via the index
// signature rather than enumerated exhaustively.
export interface TraceSpanInput extends SpanErrorLike {
  span_id: string
  parent_span_id?: string | null
  start_time_nanos: Nanos
  end_time_nanos?: Nanos
  duration_nanos?: Nanos
  // service.name is a required sort key for stored spans (see docs/architecture.md); modeled as
  // required here rather than optional so the self-time/aggregate bookkeeping below can key
  // Maps/Sets on `string` without an extra undefined-handling branch that would change behavior.
  service: string
  name: string
  [key: string]: unknown
}

// A merged/clamped BigInt [start, end) interval.
export type Interval = [bigint, bigint]

// One assembled waterfall node: the source span plus computed tree/geometry fields.
export interface TraceTreeNode {
  span: TraceSpanInput
  id: string
  parentId: string | null
  children: TraceTreeNode[]
  depth: number
  startNs: bigint
  endNs: bigint
  durationNs: bigint
  offsetNs: bigint
  onCriticalPath: boolean
  hasClockSkew: boolean
  isError: boolean
  subtreeHasError: boolean
  childCovered: Interval[]
  selfTimeNs: bigint
}

// Per-service exclusive-time rollup (see `serviceSelfTime` below).
export interface ServiceSelfTime {
  service: string
  selfNs: bigint
}

// The full assembled trace: nodes/roots/flat pre-order plus trace-level aggregates.
export interface TraceTree {
  roots: TraceTreeNode[]
  nodes: Map<string, TraceTreeNode>
  flat: TraceTreeNode[]
  startNs: bigint
  endNs: bigint
  durationNs: bigint
  spanCount: number
  serviceCount: number
  errorCount: number
  services: string[]
  criticalPath: Set<string>
  rootService: string
  rootName: string
  serviceSelfTime: ServiceSelfTime[]
}

// OTLP status: 0 UNSET, 1 OK, 2 ERROR.
export function isSpanError(span?: SpanErrorLike | null): boolean {
  return span?.status_code === 2
}

// Clamped percentage of `partNs` within `wholeNs` (both BigInt|Number). 0 when whole is 0.
export function pct(partNs: bigint | number, wholeNs: bigint | number | null | undefined): number {
  const whole = Number(wholeNs ?? 0)
  if (!whole) return 0
  const p = (Number(partNs) / whole) * 100
  return Math.max(0, Math.min(100, p))
}

function toBig(v: Nanos): bigint {
  return typeof v === 'bigint' ? v : BigInt(v ?? 0)
}

// Merge overlapping-or-touching BigInt [start, end] intervals into non-overlapping, ascending
// ranges. Empty/inverted intervals (start >= end) are dropped before merging.
export function mergeIntervals(intervals?: Interval[] | null): Interval[] {
  const valid: Interval[] = []
  for (const [s, e] of intervals ?? []) if (s < e) valid.push([s, e])
  valid.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
  const merged: Interval[] = []
  for (const [s, e] of valid) {
    const last = merged[merged.length - 1]
    if (last && s <= last[1]) {
      if (e > last[1]) last[1] = e
    } else {
      merged.push([s, e])
    }
  }
  return merged
}

export function buildTrace(spans?: TraceSpanInput[] | null): TraceTree {
  const nodes = new Map<string, TraceTreeNode>()
  for (const s of spans ?? []) {
    const startNs = toBig(s.start_time_nanos)
    let endNs: bigint
    if (s.end_time_nanos != null) endNs = toBig(s.end_time_nanos)
    else if (s.duration_nanos != null) endNs = startNs + toBig(s.duration_nanos)
    else endNs = startNs
    if (endNs < startNs) endNs = startNs
    nodes.set(s.span_id, {
      span: s,
      id: s.span_id,
      parentId: s.parent_span_id ?? null,
      children: [],
      depth: 0,
      startNs,
      endNs,
      durationNs: endNs - startNs,
      offsetNs: 0n,
      onCriticalPath: false,
      hasClockSkew: false,
      isError: isSpanError(s),
      subtreeHasError: false,
      childCovered: [],
      selfTimeNs: 0n,
    })
  }

  // Link children; a missing/self parent makes the node a root.
  const roots: TraceTreeNode[] = []
  for (const node of nodes.values()) {
    const parent = node.parentId != null ? nodes.get(node.parentId) : null
    if (parent && parent !== node) parent.children.push(node)
    else roots.push(node)
  }
  // Pure-cycle fallback (every span has an in-set parent): earliest span acts as root.
  if (roots.length === 0 && nodes.size > 0) {
    let earliest: TraceTreeNode | null = null
    for (const n of nodes.values()) if (!earliest || n.startNs < earliest.startNs) earliest = n
    if (earliest) roots.push(earliest)
  }

  // Self-time (exclusive duration): clamp each child's interval to this node's own
  // [startNs, endNs] (handles clock skew where a child starts before / ends after its parent),
  // merge overlaps so concurrent children aren't double-counted, then subtract the covered
  // length from the node's own duration. A leaf node's selfTimeNs equals its durationNs.
  for (const n of nodes.values()) {
    const clamped: Interval[] = []
    for (const c of n.children) {
      const s = c.startNs > n.startNs ? c.startNs : n.startNs
      const e = c.endNs < n.endNs ? c.endNs : n.endNs
      if (s < e) clamped.push([s, e])
    }
    n.childCovered = mergeIntervals(clamped)
    let covered = 0n
    for (const [s, e] of n.childCovered) covered += e - s
    let self = n.durationNs - covered
    if (self < 0n) self = 0n
    n.selfTimeNs = self
  }

  // Trace bounds.
  let startNs: bigint | null = null
  let endNs: bigint | null = null
  for (const n of nodes.values()) {
    if (startNs === null || n.startNs < startNs) startNs = n.startNs
    if (endNs === null || n.endNs > endNs) endNs = n.endNs
  }
  startNs = startNs ?? 0n
  endNs = endNs ?? 0n
  const durationNs = endNs > startNs ? endNs - startNs : 0n

  // Offsets (clamped) + clock-skew flag (child starts before its parent).
  for (const n of nodes.values()) {
    let off = n.startNs - startNs
    if (off < 0n) off = 0n
    n.offsetNs = off
    const parent = n.parentId != null ? nodes.get(n.parentId) : null
    if (parent && n.startNs < parent.startNs) n.hasClockSkew = true
  }

  // Sort roots + children by start time (tie by id) for stable visual order.
  const byStart = (a: TraceTreeNode, b: TraceTreeNode): number =>
    a.startNs < b.startNs ? -1 : a.startNs > b.startNs ? 1 : a.id < b.id ? -1 : 1
  roots.sort(byStart)
  for (const n of nodes.values()) n.children.sort(byStart)

  // Depth + flat pre-order (visited guard breaks cycles).
  const flat: TraceTreeNode[] = []
  const visited = new Set<string>()
  const walk = (n: TraceTreeNode, depth: number): void => {
    if (visited.has(n.id)) return
    visited.add(n.id)
    n.depth = depth
    flat.push(n)
    for (const c of n.children) walk(c, depth + 1)
  }
  for (const r of roots) walk(r, 0)
  // Defensive: any node unreached by DFS (shouldn't happen) is appended at depth 0.
  for (const n of nodes.values()) {
    if (!visited.has(n.id)) {
      n.depth = 0
      visited.add(n.id)
      flat.push(n)
    }
  }

  // subtreeHasError (memoised recursion, cycle-safe).
  const seenErr = new Set<string>()
  const computeErr = (n: TraceTreeNode): boolean => {
    if (seenErr.has(n.id)) return n.subtreeHasError
    seenErr.add(n.id)
    let e = n.isError
    for (const c of n.children) e = computeErr(c) || e
    n.subtreeHasError = e
    return e
  }
  for (const r of roots) computeErr(r)

  // Critical path: from the last-ending root, follow the last-ending child.
  let critRoot: TraceTreeNode | null = null
  for (const r of roots) if (!critRoot || r.endNs > critRoot.endNs) critRoot = r
  const criticalPath = new Set<string>()
  {
    let cur = critRoot
    const guard = new Set<string>()
    while (cur && !guard.has(cur.id)) {
      guard.add(cur.id)
      cur.onCriticalPath = true
      criticalPath.add(cur.id)
      let next: TraceTreeNode | null = null
      for (const c of cur.children) if (!next || c.endNs > next.endNs) next = c
      cur = next
    }
  }

  // Aggregates.
  const services = new Set<string>()
  let errorCount = 0
  for (const n of nodes.values()) {
    if (n.span.service) services.add(n.span.service)
    if (n.isError) errorCount++
  }
  const rootNode = roots[0] ?? null

  // Per-service self-time: sum each node's selfTimeNs by its service. Because selfTimeNs is
  // already exclusive of covered child time, nested spans of the same service aren't
  // double-counted. Sorted desc by selfNs, ties broken by service name for stable output.
  const selfByService = new Map<string, bigint>()
  for (const n of nodes.values()) {
    const svc = n.span.service
    selfByService.set(svc, (selfByService.get(svc) ?? 0n) + n.selfTimeNs)
  }
  const serviceSelfTime: ServiceSelfTime[] = [...selfByService.entries()]
    .map(([service, selfNs]) => ({ service, selfNs }))
    .sort((a, b) => {
      if (a.selfNs > b.selfNs) return -1
      if (a.selfNs < b.selfNs) return 1
      return a.service < b.service ? -1 : a.service > b.service ? 1 : 0
    })

  return {
    roots,
    nodes,
    flat,
    startNs,
    endNs,
    durationNs,
    spanCount: nodes.size,
    serviceCount: services.size,
    errorCount,
    services: [...services],
    criticalPath,
    rootService: rootNode?.span.service ?? '',
    rootName: rootNode?.span.name ?? '',
    serviceSelfTime,
  }
}

// getTraceTree(spans) — WeakMap-memoized buildTrace, keyed on the spans ARRAY
// reference. The same reference flows from the TanStack Query cache to every
// consumer (peek drawer, waterfall, detail view), so all callers share one build;
// the WeakMap lets GC drop the entry when the trace leaves the query cache. Falls
// back to a direct build for non-array input (never used as a WeakMap key).
const _treeCache = new WeakMap<TraceSpanInput[], TraceTree>()
export function getTraceTree(spans?: TraceSpanInput[] | null): TraceTree {
  if (!Array.isArray(spans)) return buildTrace(spans)
  const hit = _treeCache.get(spans)
  if (hit) return hit
  const tree = buildTrace(spans)
  _treeCache.set(spans, tree)
  return tree
}
