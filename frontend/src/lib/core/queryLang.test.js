import { describe, it, expect } from 'vitest'
import {
  tokenize,
  contextAt,
  termRangeAt,
  fieldValues,
  toggleFieldValue,
  toggleFieldValueNegated,
  removeField,
  removeFieldAll,
  durationLiteral,
  removeCompareField,
  setDurationRange,
  onlyFieldValue,
  negatedFieldValues,
  facetChecked,
  toggleFacetValue,
  fieldConstraintCount,
} from '@/lib/core/queryLang'

// Slim projection for table-driven assertions: role + text (+ negated when true, to keep
// the common case terse).
function shape(tokens) {
  return tokens.map((t) => (t.negated ? [t.role, t.text, true] : [t.role, t.text]))
}

// Every token's start/end must agree with `text`, and the full token list must be a
// contiguous, gap-free, non-overlapping cover of the input (this is what the overlay
// renderer depends on).
function assertContiguousCover(query, tokens) {
  let cursor = 0
  for (const t of tokens) {
    expect(t.start).toBe(cursor)
    expect(t.end).toBe(t.start + t.text.length)
    expect(query.slice(t.start, t.end)).toBe(t.text)
    cursor = t.end
  }
  expect(cursor).toBe(query.length)
}

describe('tokenize', () => {
  it('returns [] for empty input and coerces non-strings to empty', () => {
    expect(tokenize('')).toEqual([])
    expect(tokenize(undefined)).toEqual([])
    expect(tokenize(null)).toEqual([])
  })

  it('a whitespace-only query is a single whitespace token', () => {
    const tokens = tokenize('   ')
    expect(shape(tokens)).toEqual([['whitespace', '   ']])
    assertContiguousCover('   ', tokens)
  })

  it('field:value', () => {
    const q = 'service:api'
    const tokens = tokenize(q)
    expect(shape(tokens)).toEqual([
      ['field', 'service'],
      ['operator', ':'],
      ['value', 'api'],
    ])
    assertContiguousCover(q, tokens)
  })

  it('field:a,b OR list', () => {
    const q = 'level:info,warn'
    expect(shape(tokenize(q))).toEqual([
      ['field', 'level'],
      ['operator', ':'],
      ['value', 'info'],
      ['operator', ','],
      ['value', 'warn'],
    ])
  })

  it('field:* exists, including a dotted field name', () => {
    const q = 'host.name:*'
    expect(shape(tokenize(q))).toEqual([
      ['field', 'host.name'],
      ['operator', ':'],
      ['value', '*'],
    ])
  })

  it('numeric compare operators >, >=, <, <=', () => {
    expect(shape(tokenize('status_code>500'))).toEqual([
      ['field', 'status_code'],
      ['operator', '>'],
      ['value', '500'],
    ])
    expect(shape(tokenize('status_code>=500'))).toEqual([
      ['field', 'status_code'],
      ['operator', '>='],
      ['value', '500'],
    ])
    expect(shape(tokenize('latency_ms<10'))).toEqual([
      ['field', 'latency_ms'],
      ['operator', '<'],
      ['value', '10'],
    ])
    expect(shape(tokenize('latency_ms<=10'))).toEqual([
      ['field', 'latency_ms'],
      ['operator', '<='],
      ['value', '10'],
    ])
  })

  it('a colon inside the value is not reinterpreted as a comparison (colon wins)', () => {
    // "k:a>b" — field "k", value "a>b" (matches the Rust classify() ordering/doc comment).
    expect(shape(tokenize('k:a>b'))).toEqual([
      ['field', 'k'],
      ['operator', ':'],
      ['value', 'a>b'],
    ])
  })

  it('promoted/unknown word:value lexes exactly like a known field (lenient by design)', () => {
    expect(shape(tokenize('status_code:500'))).toEqual([
      ['field', 'status_code'],
      ['operator', ':'],
      ['value', '500'],
    ])
  })

  it('negation: the leading "-" is its own token and the whole term carries negated:true', () => {
    const q = '-level:debug'
    const tokens = tokenize(q)
    expect(shape(tokens)).toEqual([
      ['negation', '-', true],
      ['field', 'level', true],
      ['operator', ':', true],
      ['value', 'debug', true],
    ])
    expect(tokens.every((t) => t.negated)).toBe(true)
    assertContiguousCover(q, tokens)
  })

  it('negated compare and negated freetext', () => {
    expect(shape(tokenize('-status_code>=500'))).toEqual([
      ['negation', '-', true],
      ['field', 'status_code', true],
      ['operator', '>=', true],
      ['value', '500', true],
    ])
    expect(shape(tokenize('-timeout'))).toEqual([['negation', '-', true], ['freetext', 'timeout', true]])
  })

  it('quoted phrase, terminated', () => {
    const q = '"connection refused"'
    const tokens = tokenize(q)
    expect(shape(tokens)).toEqual([['quoted', '"connection refused"']])
    assertContiguousCover(q, tokens)
  })

  it('negated quoted phrase', () => {
    expect(shape(tokenize('-"timeout"'))).toEqual([
      ['negation', '-', true],
      ['quoted', '"timeout"', true],
    ])
  })

  it('unterminated quote while typing still lexes as quoted, not an error', () => {
    const q = '"connection ref'
    const tokens = tokenize(q)
    expect(shape(tokens)).toEqual([['quoted', q]])
    assertContiguousCover(q, tokens)
  })

  it('a lone "-" (nothing to negate) is a bare freetext word', () => {
    expect(shape(tokenize('-'))).toEqual([['freetext', '-']])
  })

  it('bare word is freetext', () => {
    expect(shape(tokenize('timeout'))).toEqual([['freetext', 'timeout']])
  })

  it('an empty field name before ":" degrades to freetext rather than erroring', () => {
    expect(shape(tokenize(':foo'))).toEqual([['freetext', ':foo']])
  })

  it('an empty field name before a compare operator degrades to freetext', () => {
    expect(shape(tokenize('>5'))).toEqual([['freetext', '>5']])
  })

  it('mid-typing "field:" (no value yet) is field + operator only, no dangling value token', () => {
    const q = 'level:'
    const tokens = tokenize(q)
    expect(shape(tokens)).toEqual([
      ['field', 'level'],
      ['operator', ':'],
    ])
    assertContiguousCover(q, tokens)
  })

  it('mid-typing "field>=" (no numeric value yet) is field + operator only', () => {
    expect(shape(tokenize('status_code>='))).toEqual([
      ['field', 'status_code'],
      ['operator', '>='],
    ])
  })

  it('leading/trailing/doubled commas in a value list are lenient: commas still tokenize, empty segments do not', () => {
    const q = 'level:a,,b'
    const tokens = tokenize(q)
    expect(shape(tokens)).toEqual([
      ['field', 'level'],
      ['operator', ':'],
      ['value', 'a'],
      ['operator', ','],
      ['operator', ','],
      ['value', 'b'],
    ])
    assertContiguousCover(q, tokens)

    expect(shape(tokenize('level:info,'))).toEqual([
      ['field', 'level'],
      ['operator', ':'],
      ['value', 'info'],
      ['operator', ','],
    ])

    expect(shape(tokenize('level:,info'))).toEqual([
      ['field', 'level'],
      ['operator', ':'],
      ['operator', ','],
      ['value', 'info'],
    ])
  })

  it('multiple terms separated by whitespace, including runs of whitespace', () => {
    const q = 'service:api   timeout'
    const tokens = tokenize(q)
    expect(shape(tokens)).toEqual([
      ['field', 'service'],
      ['operator', ':'],
      ['value', 'api'],
      ['whitespace', '   '],
      ['freetext', 'timeout'],
    ])
    assertContiguousCover(q, tokens)
  })

  it('leading and trailing whitespace are preserved as whitespace tokens', () => {
    const q = '  service:api  '
    const tokens = tokenize(q)
    expect(tokens[0]).toMatchObject({ role: 'whitespace', text: '  ' })
    expect(tokens.at(-1)).toMatchObject({ role: 'whitespace', text: '  ' })
    assertContiguousCover(q, tokens)
  })

  it('a realistic multi-term query covers the whole string contiguously', () => {
    const q = 'service:checkout-api,payments-api -level:debug status_code>=500 "timeout"'
    const tokens = tokenize(q)
    assertContiguousCover(q, tokens)
    // sanity: every declared role except 'negation' shows up somewhere across this fixture
    // set of tests; here just check no token role is something unexpected.
    const allowed = new Set([
      'field',
      'operator',
      'value',
      'negation',
      'freetext',
      'quoted',
      'whitespace',
    ])
    for (const t of tokens) expect(allowed.has(t.role)).toBe(true)
  })
})

describe('contextAt', () => {
  it('empty query: field context with empty prefix', () => {
    expect(contextAt('', 0)).toEqual({ kind: 'field', prefix: '' })
  })

  it('composing a bare word (no colon/operator yet) is field context', () => {
    expect(contextAt('service', 7)).toEqual({ kind: 'field', prefix: 'service' })
    expect(contextAt('serv', 4)).toEqual({ kind: 'field', prefix: 'serv' })
    expect(contextAt('status_code', 11)).toEqual({ kind: 'field', prefix: 'status_code' })
  })

  it('right after "field:" is value context with an empty prefix', () => {
    expect(contextAt('service:', 8)).toEqual({ kind: 'value', field: 'service', prefix: '' })
  })

  it('mid value is value context with the partial value as prefix', () => {
    expect(contextAt('service:ap', 10)).toEqual({ kind: 'value', field: 'service', prefix: 'ap' })
  })

  it('caret placed inside the field part of an already-complete term stays field context', () => {
    // caret after "ser" in "service:api" — still composing the field name at that position.
    expect(contextAt('service:api', 3)).toEqual({ kind: 'field', prefix: 'ser' })
  })

  it('after a comma, stays in value context for the same field (OR-list continuation)', () => {
    expect(contextAt('service:api,', 12)).toEqual({ kind: 'value', field: 'service', prefix: '' })
    expect(contextAt('service:api,we', 14)).toEqual({
      kind: 'value',
      field: 'service',
      prefix: 'we',
    })
  })

  it('negation does not change kind/field/prefix — the "-" is excluded from the field text', () => {
    expect(contextAt('-lev', 4)).toEqual({ kind: 'field', prefix: 'lev' })
    expect(contextAt('-level:de', 9)).toEqual({ kind: 'value', field: 'level', prefix: 'de' })
  })

  it('numeric compare: value context after the operator, field context before it', () => {
    expect(contextAt('status_code', 11)).toEqual({ kind: 'field', prefix: 'status_code' })
    expect(contextAt('status_code>=', 13)).toEqual({
      kind: 'value',
      field: 'status_code',
      prefix: '',
    })
    expect(contextAt('status_code>=5', 14)).toEqual({
      kind: 'value',
      field: 'status_code',
      prefix: '5',
    })
  })

  it('quoted phrase is freetext context, prefix is the typed content (no opening quote)', () => {
    expect(contextAt('"time', 5)).toEqual({ kind: 'freetext', prefix: 'time' })
    expect(contextAt('"foo bar"', 5)).toEqual({ kind: 'freetext', prefix: 'foo ' })
    expect(contextAt('"foo bar"', 1)).toEqual({ kind: 'freetext', prefix: '' })
  })

  it('caret sitting strictly inside a run of whitespace between terms falls back to empty field context', () => {
    expect(contextAt('a  b', 2)).toEqual({ kind: 'field', prefix: '' })
  })

  it('caret past the end of the string is clamped to the string length', () => {
    expect(contextAt('service:api', 999)).toEqual({ kind: 'value', field: 'service', prefix: 'api' })
  })

  it('a negative caret is clamped to 0', () => {
    expect(contextAt('service:api', -5)).toEqual({ kind: 'field', prefix: '' })
  })
})

describe('termRangeAt', () => {
  it('empty query: collapsed range at 0', () => {
    expect(termRangeAt('', 0)).toEqual({ start: 0, end: 0 })
  })

  it('caret anywhere inside a term returns the whole term range', () => {
    expect(termRangeAt('service:api', 5)).toEqual({ start: 0, end: 11 })
  })

  it('boundaries are inclusive on both ends', () => {
    expect(termRangeAt('service:api', 0)).toEqual({ start: 0, end: 11 })
    expect(termRangeAt('service:api', 11)).toEqual({ start: 0, end: 11 })
  })

  it('the leading "-" of a negated term is included in the range', () => {
    const q = '-level:debug foo'
    expect(termRangeAt(q, 3)).toEqual({ start: 0, end: 12 })
  })

  it('a quoted phrase (including internal spaces) is one term', () => {
    const q = '"a b" c'
    expect(termRangeAt(q, 2)).toEqual({ start: 0, end: 5 })
  })

  it('caret strictly inside a run of whitespace collapses to a caret-only range', () => {
    expect(termRangeAt('a  b', 2)).toEqual({ start: 2, end: 2 })
  })

  it('caret is clamped into range for out-of-bounds input', () => {
    expect(termRangeAt('service:api', 999)).toEqual({ start: 0, end: 11 })
    // clamps to 0, which is still within the first term's inclusive range
    expect(termRangeAt('service:api', -5)).toEqual({ start: 0, end: 11 })
    // a leading gap actually outside any term collapses instead
    expect(termRangeAt('  service:api', -5)).toEqual({ start: 0, end: 0 })
  })
})

describe('fieldValues', () => {
  it('returns [] for empty / non-string input and coerces non-strings', () => {
    expect(fieldValues('', 'service')).toEqual([])
    expect(fieldValues(undefined, 'service')).toEqual([])
    expect(fieldValues(null, 'service')).toEqual([])
  })

  it('returns [] when the field is absent', () => {
    expect(fieldValues('timeout "phrase"', 'service')).toEqual([])
    expect(fieldValues('level:error', 'service')).toEqual([])
  })

  it('single value', () => {
    expect(fieldValues('service:api', 'service')).toEqual(['api'])
  })

  it('OR list, order-preserving', () => {
    expect(fieldValues('service:x,y', 'service')).toEqual(['x', 'y'])
    expect(fieldValues('level:info,warn,error', 'level')).toEqual(['info', 'warn', 'error'])
  })

  it('picks out the field from a mixed query and ignores everything else', () => {
    expect(fieldValues('a service:x,y -service:z "t"', 'service')).toEqual(['x', 'y'])
  })

  it('excludes negated field terms', () => {
    expect(fieldValues('-service:z', 'service')).toEqual([])
    expect(fieldValues('service:a -service:b', 'service')).toEqual(['a'])
  })

  it('unions multiple non-negated field terms, order-preserving and deduped', () => {
    expect(fieldValues('service:a,b service:b,c', 'service')).toEqual(['a', 'b', 'c'])
  })

  it('skips empty OR segments (lenient, matching the lexer)', () => {
    expect(fieldValues('service:a,,b', 'service')).toEqual(['a', 'b'])
    expect(fieldValues('service:', 'service')).toEqual([])
  })

  it('a compare term is not a match term (never contributes values)', () => {
    expect(fieldValues('status_code>=500', 'status_code')).toEqual([])
  })
})

describe('toggleFieldValue', () => {
  it('adds to an empty query as a new term (no leading/trailing space)', () => {
    expect(toggleFieldValue('', 'service', 'api')).toBe('service:api')
  })

  it('adds a value to the existing OR list', () => {
    expect(toggleFieldValue('service:x', 'service', 'y')).toBe('service:x,y')
    expect(toggleFieldValue('service:x,y', 'service', 'z')).toBe('service:x,y,z')
  })

  it('removes a value already present in the list', () => {
    expect(toggleFieldValue('service:x,y', 'service', 'x')).toBe('service:y')
    expect(toggleFieldValue('service:x,y,z', 'service', 'y')).toBe('service:x,z')
  })

  it('removing the last value drops the whole term', () => {
    expect(toggleFieldValue('service:x', 'service', 'x')).toBe('')
  })

  it('appends a new field term when none exists yet, preserving the rest', () => {
    expect(toggleFieldValue('timeout', 'service', 'api')).toBe('timeout service:api')
  })

  it('adds to the FIRST non-negated field term when one exists', () => {
    expect(toggleFieldValue('foo service:x bar', 'service', 'y')).toBe('foo service:x,y bar')
  })

  it('preserves free text, quoted phrases, other fields and negated terms when adding', () => {
    expect(toggleFieldValue('foo service:x -level:debug "a b"', 'service', 'y')).toBe(
      'foo service:x,y -level:debug "a b"',
    )
  })

  it('collapses the whitespace left behind when a removed value empties its term', () => {
    expect(toggleFieldValue('foo   service:x   bar', 'service', 'x')).toBe('foo bar')
  })

  it('a negated field term is not the target: a new non-negated term is appended', () => {
    expect(toggleFieldValue('-service:z', 'service', 'x')).toBe('-service:z service:x')
  })

  it('preserves the internal spaces of a quoted phrase verbatim', () => {
    expect(toggleFieldValue('"a   b" service:x', 'service', 'x')).toBe('"a   b"')
  })

  it('works for the level field just the same', () => {
    expect(toggleFieldValue('service:api', 'level', 'error')).toBe('service:api level:error')
    expect(toggleFieldValue('service:api level:error,warn', 'level', 'warn')).toBe(
      'service:api level:error',
    )
  })

  it('coerces non-string queries to empty', () => {
    expect(toggleFieldValue(undefined, 'service', 'api')).toBe('service:api')
  })
})

describe('toggleFieldValueNegated', () => {
  it('appends a new -field:value term to an empty query (no leading/trailing space)', () => {
    expect(toggleFieldValueNegated('', 'service', 'api')).toBe('-service:api')
  })

  it('appends the negated term when absent, preserving the rest', () => {
    expect(toggleFieldValueNegated('timeout', 'service', 'api')).toBe('timeout -service:api')
  })

  it('removes the negated term when already present', () => {
    expect(toggleFieldValueNegated('-service:api', 'service', 'api')).toBe('')
    expect(toggleFieldValueNegated('foo -service:api bar', 'service', 'api')).toBe('foo bar')
  })

  it('preserves all other terms verbatim when adding', () => {
    expect(toggleFieldValueNegated('foo service:x "a b" -level:debug', 'service', 'y')).toBe(
      'foo service:x "a b" -level:debug -service:y',
    )
  })

  it('the negated term is distinct from the positive field:value term', () => {
    // A positive service:api is untouched; the negated term is appended alongside it.
    expect(toggleFieldValueNegated('service:api', 'service', 'api')).toBe('service:api -service:api')
    // Toggling the negated term off leaves the positive term intact.
    expect(toggleFieldValueNegated('service:api -service:api', 'service', 'api')).toBe('service:api')
  })

  it('normalises whitespace between preserved terms', () => {
    expect(toggleFieldValueNegated('foo   bar', 'service', 'api')).toBe('foo bar -service:api')
  })

  it('coerces non-string queries to empty', () => {
    expect(toggleFieldValueNegated(undefined, 'service', 'api')).toBe('-service:api')
  })
})

describe('removeField', () => {
  it('removes the non-negated field term entirely', () => {
    expect(removeField('service:x,y', 'service')).toBe('')
  })

  it('preserves the rest and normalises whitespace', () => {
    expect(removeField('foo   service:x,y   bar', 'service')).toBe('foo bar')
  })

  it('keeps other fields, free text, quoted phrases and negated terms', () => {
    expect(removeField('foo service:x -service:z level:error "a b"', 'service')).toBe(
      'foo -service:z level:error "a b"',
    )
  })

  it('removes all non-negated field terms (full clear)', () => {
    expect(removeField('service:a service:b other', 'service')).toBe('other')
  })

  it('is a no-op when the field is absent', () => {
    expect(removeField('level:error "phrase"', 'service')).toBe('level:error "phrase"')
  })

  it('coerces non-string input to empty', () => {
    expect(removeField(undefined, 'service')).toBe('')
  })
})

describe('toggleFieldValue ⟷ fieldValues round-trip (duplicate terms collapse)', () => {
  it('unchecking a value from a hand-typed multi-term query fully removes it', () => {
    // fieldValues unions across terms, so both a and b read as "checked".
    expect(fieldValues('service:a service:b', 'service')).toEqual(['a', 'b'])
    // Toggling b off must actually drop it (and collapse the duplicate term).
    const next = toggleFieldValue('service:a service:b', 'service', 'b')
    expect(next).toBe('service:a')
    expect(fieldValues(next, 'service')).toEqual(['a']) // round-trip consistent
  })

  it('adding to a duplicate-term query collapses to one canonical term', () => {
    expect(toggleFieldValue('service:a service:b', 'service', 'c')).toBe('service:a,b,c')
  })

  it('keeps the first term position and preserves surrounding terms', () => {
    expect(toggleFieldValue('foo service:a bar', 'service', 'b')).toBe('foo service:a,b bar')
  })

  it('collapsing preserves negated terms and other fields', () => {
    const next = toggleFieldValue('service:a level:error service:b -service:z', 'service', 'a')
    // a removed from the union {a,b} -> {b}; duplicates collapsed; negated kept.
    expect(next).toBe('service:b level:error -service:z')
    expect(fieldValues(next, 'service')).toEqual(['b'])
  })
})

describe('durationLiteral', () => {
  it('picks the largest unit whose mantissa is >= 1 (ns/us/ms/s boundaries)', () => {
    expect(durationLiteral(0)).toBe('0ns')
    expect(durationLiteral(1)).toBe('1ns')
    expect(durationLiteral(999)).toBe('999ns')
    expect(durationLiteral(1_000)).toBe('1us')
    expect(durationLiteral(1_500)).toBe('1.5us')
    expect(durationLiteral(1_000_000)).toBe('1ms')
    expect(durationLiteral(2_500_000)).toBe('2.5ms')
    expect(durationLiteral(500_000_000)).toBe('500ms')
    expect(durationLiteral(1_000_000_000)).toBe('1s')
    expect(durationLiteral(1_500_000_000)).toBe('1.5s')
  })

  it('rounds the mantissa to ~3 significant digits', () => {
    expect(durationLiteral(1_234_567_890)).toBe('1.23s')
    expect(durationLiteral(12_345_000)).toBe('12.3ms')
    expect(durationLiteral(123_456)).toBe('123us')
  })

  it('never emits the micro sign µs (grammar-invalid) — always us in the microsecond band', () => {
    for (const ns of [1_000, 1_500, 12_345, 500_000, 999_999]) {
      const lit = durationLiteral(ns)
      expect(lit).not.toContain('µ')
      expect(lit.endsWith('us')).toBe(true)
    }
  })

  it('coerces non-numbers to 0ns', () => {
    expect(durationLiteral(undefined)).toBe('0ns')
    expect(durationLiteral(null)).toBe('0ns')
  })
})

describe('removeCompareField', () => {
  it('removes a single compare term', () => {
    expect(removeCompareField('duration>=500ms', 'duration')).toBe('')
  })

  it('removes every compare operator form for the field', () => {
    expect(
      removeCompareField('duration>1ms duration>=2ms duration<3ms duration<=4ms', 'duration'),
    ).toBe('')
  })

  it('preserves other terms and normalises whitespace', () => {
    expect(removeCompareField('service:api   duration>=500ms   timeout', 'duration')).toBe(
      'service:api timeout',
    )
  })

  it('keeps compare terms for other fields', () => {
    expect(removeCompareField('duration>=1ms status_code>=500', 'duration')).toBe('status_code>=500')
  })

  it('keeps colon match terms, quoted phrases and negations for the same name', () => {
    expect(removeCompareField('duration:foo -level:debug "a b" duration<=9s', 'duration')).toBe(
      'duration:foo -level:debug "a b"',
    )
  })

  it('preserves a negated compare term (mirrors removeField, which the rail does not own)', () => {
    expect(removeCompareField('-duration>=1ms duration<=9s', 'duration')).toBe('-duration>=1ms')
  })

  it('is a no-op when the field has no compare term', () => {
    expect(removeCompareField('service:api "phrase"', 'duration')).toBe('service:api "phrase"')
  })

  it('coerces non-string input to empty', () => {
    expect(removeCompareField(undefined, 'duration')).toBe('')
  })
})

describe('setDurationRange', () => {
  it('appends both bounds to an empty query', () => {
    expect(setDurationRange('', 1_000_000, 500_000_000)).toBe('duration>=1ms duration<=500ms')
  })

  it('omits the min bound when minNs <= 0 (selection anchored at bucket 0)', () => {
    expect(setDurationRange('', 0, 500_000_000)).toBe('duration<=500ms')
    expect(setDurationRange('', -5, 500_000_000)).toBe('duration<=500ms')
  })

  it('replaces a prior duration range instead of stacking', () => {
    expect(
      setDurationRange('duration>=100ms duration<=200ms', 1_000_000_000, 2_000_000_000),
    ).toBe('duration>=1s duration<=2s')
  })

  it('preserves unrelated terms (service, quoted phrase, negation)', () => {
    expect(setDurationRange('service:api "slow path" -level:debug', 1_000, 2_000)).toBe(
      'service:api "slow path" -level:debug duration>=1us duration<=2us',
    )
  })

  it('never emits µs even for microsecond-scale bounds', () => {
    const out = setDurationRange('', 1_500, 12_000)
    expect(out).toBe('duration>=1.5us duration<=12us')
    expect(out).not.toContain('µ')
  })

  it('coerces a non-string query to empty before appending', () => {
    expect(setDurationRange(undefined, 0, 1_000_000)).toBe('duration<=1ms')
  })
})

describe('onlyFieldValue', () => {
  it('narrows a multi-value field to exactly one value', () => {
    expect(onlyFieldValue('service:a,b,c', 'service', 'b')).toBe('service:b')
  })
  it('replaces a different existing value', () => {
    expect(onlyFieldValue('service:a', 'service', 'b')).toBe('service:b')
  })
  it('flips an existing exclusion of the value into an only-include', () => {
    expect(onlyFieldValue('-service:b', 'service', 'b')).toBe('service:b')
  })
  it('preserves unrelated terms and other fields', () => {
    expect(onlyFieldValue('status:error service:a text', 'service', 'b')).toBe(
      'status:error text service:b',
    )
  })
  it('adds the term when the field is absent', () => {
    expect(onlyFieldValue('status:error', 'service', 'b')).toBe('status:error service:b')
  })
  it('single-state model: clears ALL of the field\'s excludes, not just an exclusion of the target value (worked example)', () => {
    // Previously onlyFieldValue only dropped a -field:value exclusion of THIS value; an
    // exclusion of a DIFFERENT value (-service:a) would survive alongside the new
    // service:b, which is the single-state bug this rework closes.
    expect(onlyFieldValue('-service:a status:error', 'service', 'b')).toBe('status:error service:b')
  })
  it('clears an unrelated -field:x exclusion of the same field (not just the target value)', () => {
    expect(onlyFieldValue('-service:x', 'service', 'b')).toBe('service:b')
  })
})

describe('removeFieldAll', () => {
  it('removes both positive and negated terms of the field', () => {
    expect(removeFieldAll('service:a -service:b status:error', 'service')).toBe('status:error')
  })

  it('is a no-op when the field is absent', () => {
    expect(removeFieldAll('status:error "phrase"', 'service')).toBe('status:error "phrase"')
  })

  it('returns empty for an empty query', () => {
    expect(removeFieldAll('', 'service')).toBe('')
  })

  it('coerces non-string input to empty', () => {
    expect(removeFieldAll(undefined, 'service')).toBe('')
  })

  it('preserves other fields, free text and quoted phrases', () => {
    expect(removeFieldAll('service:a -service:b level:error "a b" text', 'service')).toBe(
      'level:error "a b" text',
    )
  })
})

describe('facetChecked', () => {
  it('default all-mode: every value is checked when the field has no terms', () => {
    expect(facetChecked('', 'service', 'a')).toBe(true)
  })

  it('all-mode: a value excluded by -field:value reads unchecked, others stay checked', () => {
    expect(facetChecked('-service:a', 'service', 'a')).toBe(false)
    expect(facetChecked('-service:a', 'service', 'b')).toBe(true)
  })

  it('include-mode: only values present in the positive OR list are checked', () => {
    expect(facetChecked('service:a', 'service', 'b')).toBe(false)
    expect(facetChecked('service:a', 'service', 'a')).toBe(true)
  })
})

describe('toggleFacetValue', () => {
  it('all-mode: unchecking a value excludes it', () => {
    expect(toggleFacetValue('', 'service', 'a')).toBe('-service:a')
  })

  it('all-mode: re-checking an excluded value removes the exclusion (back to pristine default)', () => {
    expect(toggleFacetValue('-service:a', 'service', 'a')).toBe('')
  })

  it('include-mode: unchecking one of several includes narrows the OR list', () => {
    expect(toggleFacetValue('service:a', 'service', 'b')).toBe('service:a,b')
    expect(toggleFacetValue('service:a,b', 'service', 'a')).toBe('service:b')
  })

  it('include-mode: emptying the include set returns the field to all-mode (all checked)', () => {
    expect(toggleFacetValue('service:b', 'service', 'b')).toBe('')
  })

  it('never yields field:v AND -field:v simultaneously', () => {
    const afterUncheck = toggleFacetValue('', 'service', 'a')
    expect(fieldValues(afterUncheck, 'service')).toEqual([])
    expect(negatedFieldValues(afterUncheck, 'service')).toEqual(['a'])
  })
})

describe('fieldConstraintCount', () => {
  it('counts negated exclusions in all-mode', () => {
    expect(fieldConstraintCount('-service:a -service:b', 'service')).toBe(2)
  })

  it('counts positive includes in include-mode', () => {
    expect(fieldConstraintCount('service:a,b', 'service')).toBe(2)
  })

  it('is 0 when the field has no terms', () => {
    expect(fieldConstraintCount('status:error', 'service')).toBe(0)
  })
})

describe('negatedFieldValues', () => {
  it('returns excluded values of a field', () => {
    expect(negatedFieldValues('-service:a -service:b', 'service')).toEqual(['a', 'b'])
  })
  it('ignores positive terms and other fields', () => {
    expect(negatedFieldValues('service:a -kind:client', 'service')).toEqual([])
  })
  it('returns [] for absent field', () => {
    expect(negatedFieldValues('text only', 'service')).toEqual([])
  })
})
