import { describe, it, expect } from 'vitest'
import { namespaceOf, groupByNamespace, rankMetrics } from '@/lib/metrics/metricNamespaces'

const E = (name: string) => ({ name })

describe('namespaceOf', () => {
  it('splits on the first dot or underscore', () => {
    expect(namespaceOf('http.server.duration')).toBe('http')
    expect(namespaceOf('http_requests_total')).toBe('http')
  })
  it('returns null when there is no separator', () => {
    expect(namespaceOf('uptime')).toBeNull()
  })
})

describe('groupByNamespace', () => {
  it('groups 2+ metrics sharing a prefix and sorts groups + members by name', () => {
    const groups = groupByNamespace([E('http.b'), E('http.a'), E('db.query'), E('db.conn')])
    expect(groups.map((g) => g.name)).toEqual(['db', 'http'])
    expect(groups[1].metrics.map((m) => m.name)).toEqual(['http.a', 'http.b'])
  })
  it('collapses singleton namespaces and separator-less names into the trailing "" (Other) group', () => {
    const groups = groupByNamespace([E('http.a'), E('http.b'), E('lonely.one'), E('uptime')])
    const other = groups.find((g) => g.name === '')
    expect(other?.metrics.map((m) => m.name).sort()).toEqual(['lonely.one', 'uptime'])
    expect(groups[groups.length - 1].name).toBe('') // Other sorts last
  })
})

describe('rankMetrics', () => {
  it('returns all sorted when the query is empty', () => {
    expect(rankMetrics([E('b'), E('a')], '').map((m) => m.name)).toEqual(['a', 'b'])
  })
  it('ranks prefix < word-boundary < substring and drops non-matches', () => {
    const r = rankMetrics(
      [E('zzz'), E('a.server'), E('server.x'), E('myserver')],
      'server',
    ).map((m) => m.name)
    expect(r).toEqual(['server.x', 'a.server', 'myserver']) // prefix, then boundary, then substring; 'zzz' dropped
  })
})
