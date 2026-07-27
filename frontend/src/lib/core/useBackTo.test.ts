import { describe, it, expect, vi, afterEach } from 'vitest'
import { isListEntry, backTo } from './useBackTo'

describe('isListEntry', () => {
  it('matches the bare list path', () => {
    expect(isListEntry('/traces', '/traces')).toBe(true)
  })

  it('matches the list path carrying filters', () => {
    expect(isListEntry('/traces?q=status%3Aerror&sort=slowest&mode=spans&range=30m', '/traces')).toBe(true)
  })

  it('matches with a hash', () => {
    expect(isListEntry('/traces#top', '/traces')).toBe(true)
  })

  it('rejects a detail route under the list path', () => {
    expect(isListEntry('/traces/abc123', '/traces')).toBe(false)
  })

  it('rejects a different route sharing the prefix', () => {
    expect(isListEntry('/traces-archive', '/traces')).toBe(false)
  })

  it('rejects a foreign route', () => {
    expect(isListEntry('/logs?q=trace_id%3Aabc', '/traces')).toBe(false)
  })

  it('rejects a missing/deep-linked entry', () => {
    expect(isListEntry(null, '/traces')).toBe(false)
    expect(isListEntry(undefined, '/traces')).toBe(false)
    expect(isListEntry('', '/traces')).toBe(false)
  })
})

describe('backTo', () => {
  const origState = Object.getOwnPropertyDescriptor(History.prototype, 'state')
  afterEach(() => {
    if (origState) Object.defineProperty(History.prototype, 'state', origState)
  })
  function stubHistoryState(state: unknown) {
    Object.defineProperty(History.prototype, 'state', { configurable: true, get: () => state })
  }

  it('returns to the previous entry when it is the list (keeping its filters)', () => {
    stubHistoryState({ back: '/traces?q=status%3Aerror&sort=slowest' })
    const router = { back: vi.fn(), push: vi.fn() }
    backTo(router as never, '/traces')
    expect(router.back).toHaveBeenCalled()
    expect(router.push).not.toHaveBeenCalled()
  })

  it('pushes the list path when arriving from elsewhere', () => {
    stubHistoryState({ back: '/logs?q=trace_id%3Aabc' })
    const router = { back: vi.fn(), push: vi.fn() }
    backTo(router as never, '/traces')
    expect(router.push).toHaveBeenCalledWith('/traces')
    expect(router.back).not.toHaveBeenCalled()
  })

  it('pushes the list path on a deep link (no previous entry)', () => {
    stubHistoryState({})
    const router = { back: vi.fn(), push: vi.fn() }
    backTo(router as never, '/traces')
    expect(router.push).toHaveBeenCalledWith('/traces')
  })
})
