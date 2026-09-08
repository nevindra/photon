// Find-in-details for the container drawer. An operator opening Details is usually hunting one
// string — an env var name, a mount path, a label value — in a list of a hundred rows, so the
// search is a plain case-insensitive substring over key AND value, every hit highlighted, rows
// that miss hidden. Pure so the table tests can run it without a DOM.

export interface Segment {
  text: string
  hit: boolean
}

/** What the input typed, normalized for matching: trimmed, case-folded. '' means "no filter". */
export function normalizeQuery(q: string | null | undefined): string {
  return (q ?? '').trim().toLowerCase()
}

export function contains(text: string | null | undefined, q: string): boolean {
  if (!q) return true
  return (text ?? '').toLowerCase().includes(q)
}

/** Split `text` into hit / miss segments for every occurrence of `q` (already normalized). */
export function splitHits(text: string, q: string): Segment[] {
  if (!q || !text) return [{ text, hit: false }]
  const lower = text.toLowerCase()
  const out: Segment[] = []
  let i = 0
  for (;;) {
    const at = lower.indexOf(q, i)
    if (at < 0) break
    if (at > i) out.push({ text: text.slice(i, at), hit: false })
    out.push({ text: text.slice(at, at + q.length), hit: true })
    i = at + q.length
  }
  if (i < text.length) out.push({ text: text.slice(i), hit: false })
  return out.length ? out : [{ text, hit: false }]
}

/** A row survives the filter when any of its searchable strings contains the query. */
export function rowMatches(fields: Array<string | null | undefined>, q: string): boolean {
  if (!q) return true
  return fields.some((f) => contains(f, q))
}
