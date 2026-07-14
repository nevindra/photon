import { describe, it, expect } from 'vitest'
import { FIELDS, findField, EXAMPLE_QUERIES } from '@/lib/logs/fields'
import { SEVERITIES } from '@/lib/core/format'
import { tokenize } from '@/lib/core/queryLang'

describe('FIELDS catalog', () => {
  it('is exactly the five fixed fields, in a stable order', () => {
    expect(FIELDS.map((f) => f.name)).toEqual([
      'service',
      'level',
      'severity_text',
      'trace_id',
      'span_id',
    ])
  })

  it('every entry has the {name, description, kind} shape and a non-empty description', () => {
    for (const f of FIELDS) {
      expect(typeof f.name).toBe('string')
      expect(f.name.length).toBeGreaterThan(0)
      expect(typeof f.description).toBe('string')
      expect(f.description.length).toBeGreaterThan(0)
      expect(['match', 'compare']).toContain(f.kind)
    }
  })

  it('promoted/long-tail attributes (e.g. status_code) are intentionally absent from the catalog', () => {
    expect(findField('status_code')).toBeUndefined()
  })

  it('service is a match field whose values come from the live services list', () => {
    const service = findField('service')
    expect(service.kind).toBe('match')
    expect(service.values).toBe('services')
  })

  it('level is a match field with the fixed debug/info/warn/error/fatal enum, in severity order', () => {
    const level = findField('level')
    expect(level.kind).toBe('match')
    expect(level.values).toEqual(['debug', 'info', 'warn', 'error', 'fatal'])
    // stays in sync with the single source of truth for severity ordering
    expect(level.values).toEqual(SEVERITIES.map((s) => s.key))
  })

  it('severity_text, trace_id, span_id are match fields with no fixed value list (free entry)', () => {
    for (const name of ['severity_text', 'trace_id', 'span_id']) {
      const f = findField(name)
      expect(f.kind).toBe('match')
      expect(f.values).toBeUndefined()
    }
  })
})

describe('findField', () => {
  it('returns undefined for unknown field names', () => {
    expect(findField('nope')).toBeUndefined()
    expect(findField('')).toBeUndefined()
  })

  it('returns the exact catalog entry by name', () => {
    expect(findField('trace_id')).toBe(FIELDS.find((f) => f.name === 'trace_id'))
  })
})

describe('EXAMPLE_QUERIES', () => {
  it('has 2-3 realistic example queries', () => {
    expect(EXAMPLE_QUERIES.length).toBeGreaterThanOrEqual(2)
    expect(EXAMPLE_QUERIES.length).toBeLessThanOrEqual(3)
    for (const q of EXAMPLE_QUERIES) {
      expect(typeof q).toBe('string')
      expect(q.trim().length).toBeGreaterThan(0)
    }
  })

  it('every example is well-formed enough that the lexer produces real (non-whitespace-only) tokens', () => {
    for (const q of EXAMPLE_QUERIES) {
      const tokens = tokenize(q)
      expect(tokens.some((t) => t.role !== 'whitespace')).toBe(true)
    }
  })
})
