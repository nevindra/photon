import { describe, it, expect, vi, beforeEach } from 'vitest'
import { openLiveStream } from '@/lib/core/liveStream'

class MockES {
  constructor(url) { this.url = url; this.listeners = {}; MockES.last = this }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn) }
  emit(type, data) { (this.listeners[type] ?? []).forEach(fn => fn({ data })) }
  close() { this.closed = true }
}

beforeEach(() => { global.EventSource = MockES })

describe('openLiveStream', () => {
  it('encodes the query into the URL for the grain', () => {
    openLiveStream({ grain: 'spans', query: 'service:pay a b', onRows(){} })
    expect(MockES.last.url).toBe('/api/stream/spans?q=service%3Apay%20a%20b')
  })

  it('routes rows / lag / rate / status callbacks', () => {
    const onRows = vi.fn(), onLag = vi.fn(), onRate = vi.fn(), onStatus = vi.fn()
    const h = openLiveStream({ grain: 'logs', query: '', onRows, onLag, onRate, onStatus })
    MockES.last.emit('open')
    MockES.last.emit('rows', JSON.stringify([{ id: 1, service: 'x', timestamp: '1751000000000000000' }]))
    MockES.last.emit('lag', JSON.stringify({ skipped: 3 }))
    MockES.last.emit('rate', JSON.stringify({ matched_per_sec: 42 }))
    MockES.last.emit('error')
    expect(onStatus).toHaveBeenCalledWith('live')
    // Rows are hydrated into the SAME UI shape the search path uses: string nanos → BigInt, so a
    // merged table of streamed + searched rows never mixes BigInt and string in time formatting.
    expect(onRows).toHaveBeenCalledWith([{ id: 1, service: 'x', timestamp: 1751000000000000000n }])
    expect(onLag).toHaveBeenCalledWith(3)
    expect(onRate).toHaveBeenCalledWith(42)
    expect(onStatus).toHaveBeenCalledWith('reconnecting')
    h.close()
    expect(MockES.last.closed).toBe(true)
  })

  it('hydrates streamed spans (start/end nanos → BigInt) like the search path', () => {
    const onRows = vi.fn()
    openLiveStream({ grain: 'spans', query: '', onRows })
    MockES.last.emit('rows', JSON.stringify([
      { id: 7, span_id: 'abc', start_time_nanos: '1751000000000000000', end_time_nanos: '1751000000005000000' },
    ]))
    expect(onRows).toHaveBeenCalledWith([
      { id: 7, span_id: 'abc', start_time_nanos: 1751000000000000000n, end_time_nanos: 1751000000005000000n },
    ])
  })
})
