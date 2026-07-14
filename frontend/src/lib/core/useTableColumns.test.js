import { describe, it, expect, beforeEach } from 'vitest'
import { useTableColumns } from '@/lib/core/useTableColumns'

describe('useTableColumns', () => {
  beforeEach(() => localStorage.clear())

  it('persists visibility + attr columns under the table key', () => {
    const a = useTableColumns('spans', { builtins: [{ key: 'status', label: 'Status' }] })
    a.addAttr('http.route')
    a.toggleBuiltin('status') // hide it
    const b = useTableColumns('spans', { builtins: [{ key: 'status', label: 'Status' }] })
    expect(b.attrColumns.value).toContain('http.route')
    expect(b.isVisible('status')).toBe(false)
  })

  it('seeds visibleKeys from builtins minus hidden', () => {
    const c = useTableColumns('t1', {
      builtins: [
        { key: 'a', label: 'A' },
        { key: 'b', label: 'B' },
      ],
    })
    expect(c.visibleKeys.value).toEqual(['a', 'b'])
    c.toggleBuiltin('a')
    expect(c.visibleKeys.value).toEqual(['b'])
    expect(c.isVisible('a')).toBe(false)
    c.toggleBuiltin('a') // show again
    expect(c.visibleKeys.value).toEqual(['a', 'b'])
    expect(c.isVisible('a')).toBe(true)
  })

  it('addAttr is idempotent and removeAttr drops the column', () => {
    const d = useTableColumns('t2', { builtins: [] })
    d.addAttr('region')
    d.addAttr('region')
    expect(d.attrColumns.value).toEqual(['region'])
    d.removeAttr('region')
    expect(d.attrColumns.value).toEqual([])
  })
})
