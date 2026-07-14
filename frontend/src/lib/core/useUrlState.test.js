import { describe, it, expect } from 'vitest'
import { parseQuery, buildQuery } from '@/lib/core/useUrlState'

describe('parseQuery', () => {
  it('returns defaults for an empty/missing search string', () => {
    expect(parseQuery('')).toEqual({ services: [], severities: [], timeRange: null, text: '' })
    expect(parseQuery(undefined)).toEqual({
      services: [],
      severities: [],
      timeRange: null,
      text: '',
    })
  })

  it('parses comma-joined svc/sev lists, range, and q', () => {
    expect(parseQuery('?svc=api,web&sev=warn,error&range=1h&q=timeout')).toEqual({
      services: ['api', 'web'],
      severities: ['warn', 'error'],
      timeRange: '1h',
      text: 'timeout',
    })
  })

  it('works without a leading "?"', () => {
    expect(parseQuery('svc=api&range=15m')).toEqual({
      services: ['api'],
      severities: [],
      timeRange: '15m',
      text: '',
    })
  })

  it('trims and drops empty entries from list values', () => {
    expect(parseQuery('?svc=api,, web ,')).toEqual({
      services: ['api', 'web'],
      severities: [],
      timeRange: null,
      text: '',
    })
  })
})

describe('buildQuery', () => {
  it('returns "" when every field is empty/default', () => {
    expect(buildQuery({ services: [], severities: [], timeRange: null, text: '' })).toBe('')
    expect(buildQuery({})).toBe('')
    expect(buildQuery()).toBe('')
  })

  it('omits empty fields and only includes set ones', () => {
    expect(buildQuery({ services: ['api'], severities: [], timeRange: null, text: '' })).toBe(
      '?svc=api',
    )
  })

  it('joins lists with commas and includes range/text', () => {
    const qs = buildQuery({
      services: ['api', 'web'],
      severities: ['warn', 'error'],
      timeRange: '1h',
      text: 'timeout',
    })
    expect(qs).toBe('?svc=api%2Cweb&sev=warn%2Cerror&range=1h&q=timeout')
  })
})

describe('round-trip', () => {
  const cases = [
    { services: [], severities: [], timeRange: null, text: '' },
    { services: ['api'], severities: ['error'], timeRange: '30m', text: '' },
    { services: ['api', 'web', 'auth'], severities: ['warn', 'error', 'fatal'], timeRange: '24h', text: 'db timeout' },
    { services: [], severities: [], timeRange: '15m', text: '' },
    { services: [], severities: [], timeRange: null, text: 'hello world' },
  ]

  it('parseQuery(buildQuery(state)) === state for a range of states', () => {
    for (const state of cases) {
      expect(parseQuery(buildQuery(state))).toEqual(state)
    }
  })

  it('is stable under a second round-trip (idempotent)', () => {
    for (const state of cases) {
      const once = parseQuery(buildQuery(state))
      const twice = parseQuery(buildQuery(once))
      expect(twice).toEqual(once)
    }
  })
})
