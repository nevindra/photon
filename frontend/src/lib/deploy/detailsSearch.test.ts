import { describe, it, expect } from 'vitest'
import { normalizeQuery, contains, splitHits, rowMatches } from './detailsSearch'

describe('detailsSearch', () => {
  it('normalizes the query and matches case-insensitively', () => {
    expect(normalizeQuery('  DB_Host ')).toBe('db_host')
    expect(contains('DB_HOST', 'db_host')).toBe(true)
    expect(contains('postgres', 'db_host')).toBe(false)
    expect(contains(null, 'x')).toBe(false)
    expect(contains(null, '')).toBe(true)
  })

  it('splits every occurrence into highlighted segments', () => {
    expect(splitHits('PHOTON_OTLP_HTTP_ENDPOINT', 'otlp')).toEqual([
      { text: 'PHOTON_', hit: false },
      { text: 'OTLP', hit: true },
      { text: '_HTTP_ENDPOINT', hit: false },
    ])
    expect(splitHits('aXbXc', 'x')).toEqual([
      { text: 'a', hit: false },
      { text: 'X', hit: true },
      { text: 'b', hit: false },
      { text: 'X', hit: true },
      { text: 'c', hit: false },
    ])
    expect(splitHits('nothing', 'zz')).toEqual([{ text: 'nothing', hit: false }])
    expect(splitHits('plain', '')).toEqual([{ text: 'plain', hit: false }])
  })

  it('keeps a row when any field contains the query', () => {
    expect(rowMatches(['DB_HOST', 'db'], 'host')).toBe(true)
    expect(rowMatches(['DB_HOST', 'db'], 'postgres')).toBe(false)
    expect(rowMatches(['anything'], '')).toBe(true)
  })
})
