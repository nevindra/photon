// A line diff for the deploy preview: what changes in the rendered compose between the generation
// a host runs and the one about to ship. LCS-based, fine for compose files (hundreds of lines).
export interface DiffLine {
  type: 'same' | 'add' | 'del'
  text: string
}

export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before.split('\n')
  const b = after.split('\n')
  if (a.length === 1 && a[0] === '' && before === '') return b.map((text) => ({ type: 'add', text }))
  const n = a.length
  const m = b.length
  // lcs[i][j] = length of LCS of a[i..], b[j..]
  const lcs: Uint16Array[] = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1))
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1])
    }
  }
  const out: DiffLine[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push({ type: 'same', text: a[i] })
      i++
      j++
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ type: 'del', text: a[i] })
      i++
    } else {
      out.push({ type: 'add', text: b[j] })
      j++
    }
  }
  while (i < n) out.push({ type: 'del', text: a[i++] })
  while (j < m) out.push({ type: 'add', text: b[j++] })
  return out
}

export function diffStats(lines: DiffLine[]): { added: number; removed: number } {
  return { added: lines.filter((l) => l.type === 'add').length, removed: lines.filter((l) => l.type === 'del').length }
}

/** Collapse long runs of unchanged lines to `context` lines around each change (with a marker). */
export function collapseDiff(lines: DiffLine[], context = 3): (DiffLine | { type: 'skip'; count: number })[] {
  const keep = new Array<boolean>(lines.length).fill(false)
  lines.forEach((l, i) => {
    if (l.type !== 'same') for (let k = Math.max(0, i - context); k <= Math.min(lines.length - 1, i + context); k++) keep[k] = true
  })
  if (!lines.some((l) => l.type !== 'same')) return lines
  const out: (DiffLine | { type: 'skip'; count: number })[] = []
  let skipping = 0
  lines.forEach((l, i) => {
    if (keep[i]) {
      if (skipping) out.push({ type: 'skip', count: skipping })
      skipping = 0
      out.push(l)
    } else skipping++
  })
  if (skipping) out.push({ type: 'skip', count: skipping })
  return out
}
