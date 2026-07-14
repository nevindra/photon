import { describe, it, expect } from 'vitest'
import { SPAN_FIELDS, findSpanField, SPAN_EXAMPLE_QUERIES } from '@/lib/traces/spanFields'
import { tokenize } from '@/lib/core/queryLang'

describe('SPAN_FIELDS catalog', () => {
  it('is exactly the eight fixed span fields, in a stable order', () => {
    expect(SPAN_FIELDS.map((f) => f.name)).toEqual([
      'service',
      'operation',
      'status',
      'kind',
      'duration',
      'trace_id',
      'span_id',
      'parent_span_id',
    ])
  })

  it('every entry has the {name, description, kind} shape and a non-empty description', () => {
    for (const f of SPAN_FIELDS) {
      expect(typeof f.name).toBe('string')
      expect(f.name.length).toBeGreaterThan(0)
      expect(typeof f.description).toBe('string')
      expect(f.description.length).toBeGreaterThan(0)
      expect(['match', 'compare']).toContain(f.kind)
    }
  })

  it('catalog names are unique', () => {
    const names = SPAN_FIELDS.map((f) => f.name)
    expect(new Set(names).size).toBe(names.length)
  })

  it('service is a match field whose values come from the live services list', () => {
    const service = findSpanField('service')
    expect(service.kind).toBe('match')
    expect(service.values).toBe('services')
  })

  it('operation is a match field with no fixed value list (free entry)', () => {
    const operation = findSpanField('operation')
    expect(operation.kind).toBe('match')
    expect(operation.values).toBeUndefined()
  })

  it('status is a match field with the ok/error/unset enum', () => {
    const status = findSpanField('status')
    expect(status.kind).toBe('match')
    expect(status.values).toEqual(['ok', 'error', 'unset'])
  })

  it('kind is a match field with the server/client/internal/producer/consumer enum', () => {
    const kind = findSpanField('kind')
    expect(kind.kind).toBe('match')
    expect(kind.values).toEqual(['server', 'client', 'internal', 'producer', 'consumer'])
  })

  it('duration is a compare field with no fixed value list', () => {
    const duration = findSpanField('duration')
    expect(duration.kind).toBe('compare')
    expect(duration.values).toBeUndefined()
  })

  it('trace_id, span_id, parent_span_id are match fields with no fixed value list (free entry)', () => {
    for (const name of ['trace_id', 'span_id', 'parent_span_id']) {
      const f = findSpanField(name)
      expect(f.kind).toBe('match')
      expect(f.values).toBeUndefined()
    }
  })
})

describe('findSpanField', () => {
  it('returns undefined for unknown field names', () => {
    expect(findSpanField('nope')).toBeUndefined()
    expect(findSpanField('')).toBeUndefined()
  })

  it('returns the exact catalog entry by name', () => {
    expect(findSpanField('trace_id')).toBe(SPAN_FIELDS.find((f) => f.name === 'trace_id'))
  })
})

describe('SPAN_EXAMPLE_QUERIES', () => {
  it('is exactly the three documented example queries', () => {
    expect(SPAN_EXAMPLE_QUERIES).toEqual([
      'service:checkout status:error',
      'duration>=500ms',
      'operation:charge.card kind:client',
    ])
  })

  it('every example is well-formed enough that the lexer produces real (non-whitespace-only) tokens', () => {
    for (const q of SPAN_EXAMPLE_QUERIES) {
      const tokens = tokenize(q)
      expect(tokens.some((t) => t.role !== 'whitespace')).toBe(true)
    }
  })
})
