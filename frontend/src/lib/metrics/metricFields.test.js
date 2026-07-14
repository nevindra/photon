import { describe, it, expect } from 'vitest'
import {
  buildMetricCatalog, aggOptionsForType, defaultAggForType, isChartable, groupByDisabled, AGG_OPTIONS,
  METRIC_EXAMPLE_QUERIES,
} from '@/lib/metrics/metricFields'

describe('metricFields', () => {
  it('builds a SearchBar catalog from attribute keys, wiring service to the services list', () => {
    const cat = buildMetricCatalog(['service', 'http.route', 'deployment.environment'], ['checkout'])
    const svc = cat.find((f) => f.name === 'service')
    expect(svc.values).toBe('services')
    expect(cat.map((f) => f.name)).toEqual(
      expect.arrayContaining(['service', 'http.route', 'deployment.environment']),
    )
    expect(cat.every((f) => f.kind === 'match')).toBe(true)
  })
  it('offers type-appropriate aggregations with the smart default first', () => {
    expect(aggOptionsForType('gauge', null)[0]).toBe('avg')
    expect(aggOptionsForType('sum', true)[0]).toBe('rate')
    expect(aggOptionsForType('sum', false)[0]).toBe('sum')
    expect(aggOptionsForType('gauge', null)).toEqual(expect.arrayContaining(['avg', 'min', 'max', 'last']))
  })
  it('mirrors the server smart default', () => {
    expect(defaultAggForType('gauge', null)).toBe('avg')
    expect(defaultAggForType('sum', true)).toBe('rate')
    expect(defaultAggForType('sum', false)).toBe('sum')
    expect(defaultAggForType('histogram', null)).toBe('p99')
    expect(defaultAggForType('summary', null)).toBe('median')
  })
  it('charts all five metric types', () => {
    expect(isChartable('gauge')).toBe(true)
    expect(isChartable('sum')).toBe(true)
    expect(isChartable('histogram')).toBe(true)
    expect(isChartable('exp_histogram')).toBe(true)
    expect(isChartable('summary')).toBe(true)
    expect(isChartable('unknown')).toBe(false)
  })
  it('disables group-by only for summary', () => {
    expect(groupByDisabled('summary')).toBe(true)
    expect(groupByDisabled('histogram')).toBe(false)
    expect(groupByDisabled('gauge')).toBe(false)
  })
  it('every agg option has a human label', () => {
    for (const a of aggOptionsForType('gauge', null)) expect(AGG_OPTIONS[a]).toBeTruthy()
    expect(METRIC_EXAMPLE_QUERIES.length).toBeGreaterThan(0)
  })
})
