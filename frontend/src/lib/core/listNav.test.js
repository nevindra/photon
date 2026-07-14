import { describe, it, expect } from 'vitest'
import { nextIndex } from '@/lib/core/listNav'

describe('nextIndex', () => {
  it('returns -1 for an empty list regardless of current index or delta', () => {
    expect(nextIndex(0, -1, 1)).toBe(-1)
    expect(nextIndex(0, -1, -1)).toBe(-1)
    expect(nextIndex(0, 3, 1)).toBe(-1)
  })

  it('with no selection (-1), lands on the first item for a positive delta', () => {
    expect(nextIndex(5, -1, 1)).toBe(0)
  })

  it('with no selection (-1), lands on the last item for a negative delta', () => {
    expect(nextIndex(5, -1, -1)).toBe(4)
  })

  it('clamps at the start: index 0 with delta -1 stays 0', () => {
    expect(nextIndex(5, 0, -1)).toBe(0)
  })

  it('clamps at the end: last index with delta +1 stays at the last index', () => {
    expect(nextIndex(5, 4, 1)).toBe(4)
  })

  it('steps normally within bounds', () => {
    expect(nextIndex(5, 2, 1)).toBe(3)
    expect(nextIndex(5, 2, -1)).toBe(1)
  })
})
