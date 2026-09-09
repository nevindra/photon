import { describe, it, expect } from 'vitest'
import { lineDiff, diffStats, collapseDiff } from './diff'

describe('lineDiff', () => {
  it('marks added, removed and unchanged lines', () => {
    const d = lineDiff('a\nb\nc', 'a\nx\nc\nd')
    expect(d).toEqual([{ type: 'same', text: 'a' }, { type: 'del', text: 'b' }, { type: 'add', text: 'x' }, { type: 'same', text: 'c' }, { type: 'add', text: 'd' }])
    expect(diffStats(d)).toEqual({ added: 2, removed: 1 })
  })
  it('treats an empty before as all additions', () => {
    expect(lineDiff('', 'a\nb').every((l) => l.type === 'add')).toBe(true)
  })
  it('collapses long unchanged runs around changes', () => {
    const before = Array.from({ length: 20 }, (_, i) => `l${i}`).join('\n')
    const after = before.replace('l10', 'L10')
    const c = collapseDiff(lineDiff(before, after), 2)
    expect(c.some((l) => l.type === 'skip')).toBe(true)
    expect(c.filter((l) => l.type !== 'skip').length).toBeLessThan(20)
  })
})
