import { ref, nextTick } from 'vue'
import { describe, it, expect, vi, beforeEach } from 'vitest'

let streamHandle
vi.mock('@/lib/core/liveStream', () => ({
  openLiveStream: vi.fn((opts) => { streamHandle = { opts, close: vi.fn() }; return streamHandle }),
}))
import { openLiveStream } from '@/lib/core/liveStream'
import { useLiveTail, mergeLiveRows } from '@/lib/core/useLiveTail'

beforeEach(() => { openLiveStream.mockClear(); streamHandle = null })

// Streamed rows are tagged with a client-unique, never-reset `id` (`live-*`) so they never collide
// with the search page's positional numeric ids (0,1,2…) nor reset on EventSource reconnect. So the
// tests assert order/cap/pause on a STABLE marker field (`n`) and separately assert the ids are
// unique `live-*` strings.
describe('useLiveTail', () => {
  it('prepends streamed rows newest-first and caps the buffer', async () => {
    const lt = useLiveTail({ grain: 'logs', query: ref(''), bufferCap: 3 })
    lt.setMode('live')
    streamHandle.opts.onRows([{ n: 1 }, { n: 2 }])
    streamHandle.opts.onRows([{ n: 3 }, { n: 4 }])
    await nextTick()
    expect(lt.rows.value.map(r => r.n)).toEqual([4, 3, 2]) // newest-first, capped at 3
    const ids = lt.rows.value.map(r => r.id)
    expect(new Set(ids).size).toBe(ids.length) // unique
    expect(ids.every(id => typeof id === 'string' && id.startsWith('live-'))).toBe(true)
  })

  it('pauses prepend and counts new rows, then jumpToLatest flushes', async () => {
    const lt = useLiveTail({ grain: 'logs', query: ref('') })
    lt.setMode('live')
    lt.setPaused(true)
    streamHandle.opts.onRows([{ n: 1 }, { n: 2 }])
    await nextTick()
    expect(lt.rows.value).toEqual([])
    expect(lt.newCount.value).toBe(2)
    lt.jumpToLatest()
    await nextTick()
    expect(lt.rows.value.map(r => r.n)).toEqual([2, 1])
    expect(lt.newCount.value).toBe(0)
    expect(lt.paused.value).toBe(false)
    const ids = lt.rows.value.map(r => r.id)
    expect(ids.every(id => typeof id === 'string' && id.startsWith('live-'))).toBe(true)
  })

  it('reopens the stream and clears rows when the query changes', async () => {
    const query = ref('a')
    const lt = useLiveTail({ grain: 'logs', query })
    lt.setMode('live')
    streamHandle.opts.onRows([{ n: 1 }])
    await nextTick()
    query.value = 'b'
    await nextTick()
    expect(streamHandle.opts.query).toBe('b')      // reopened with new query
    expect(lt.rows.value).toEqual([])              // buffer cleared
    expect(openLiveStream).toHaveBeenCalledTimes(2)
  })

  it('never restarts streamed ids across a reconnect (uid is composable-scoped)', async () => {
    const query = ref('a')
    const lt = useLiveTail({ grain: 'logs', query })
    lt.setMode('live')
    streamHandle.opts.onRows([{ n: 1 }])
    await nextTick()
    const firstId = lt.rows.value[0].id
    query.value = 'b'                              // triggers reopen -> rows cleared, uid NOT reset
    await nextTick()
    streamHandle.opts.onRows([{ n: 2 }])
    await nextTick()
    expect(lt.rows.value[0].id).not.toBe(firstId)  // did NOT restart at live-0 and re-collide
  })

  it('live resolves to poll when not streamable (metrics)', () => {
    const onPoll = vi.fn()
    const lt = useLiveTail({ grain: 'metrics', query: ref(''), streamable: false, onPoll })
    lt.setMode('live')
    expect(openLiveStream).not.toHaveBeenCalled()
    expect(onPoll).toHaveBeenCalledWith(2000)
  })
})

// Reproduction test for the Live-mode blank-table bug: the table showed the (empty) live buffer
// instead of merging streamed rows on top of the frozen search page. mergeLiveRows is that merge.
describe('mergeLiveRows', () => {
  it('returns the base when nothing has streamed yet (Live must NOT blank the table)', () => {
    const base = [{ id: 0, body: 'a' }, { id: 1, body: 'b' }]
    expect(mergeLiveRows([], base)).toEqual(base)
  })

  it('prepends streamed rows above the base (newest on top)', () => {
    const streamed = [{ id: 'live-1' }, { id: 'live-0' }] // already newest-first
    const base = [{ id: 0 }, { id: 1 }]
    expect(mergeLiveRows(streamed, base).map(r => r.id)).toEqual(['live-1', 'live-0', 0, 1])
  })

  it('does not cross-drop between the two id spaces (streamed live-0 and base numeric 0 both survive)', () => {
    const streamed = [{ id: 'live-0' }]
    const base = [{ id: 0 }]
    expect(mergeLiveRows(streamed, base).map(r => String(r.id))).toEqual(['live-0', '0'])
  })

  it('respects the cap', () => {
    const streamed = Array.from({ length: 5 }, (_, i) => ({ id: `live-${i}` }))
    const base = Array.from({ length: 5 }, (_, i) => ({ id: i }))
    const out = mergeLiveRows(streamed, base, 3)
    expect(out).toHaveLength(3)
    expect(out.map(r => r.id)).toEqual(['live-0', 'live-1', 'live-2'])
  })

  it('dedups by id defensively (streamed wins over a same-id base row)', () => {
    const streamed = [{ id: 'live-0', fresh: true }]
    const base = [{ id: 'live-0', stale: true }, { id: 0 }]
    const out = mergeLiveRows(streamed, base)
    expect(out.map(r => String(r.id))).toEqual(['live-0', '0'])
    expect(out[0].fresh).toBe(true)
  })

  it('tolerates a null/undefined base', () => {
    expect(mergeLiveRows([{ id: 'live-0' }], null)).toEqual([{ id: 'live-0' }])
  })
})
