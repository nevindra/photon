import { describe, it, expect } from 'vitest'
import {
  latestValue, latestTotal, worstSeries, utilAccent, sparkValues, cpuSeriesForMode, formatPct,
  hostStatus,
} from './hostStats'
import type { SeriesLike } from './hostStats'

const s = (labels: Record<string, string | null>, vs: (number | null)[]): SeriesLike => ({
  labels,
  points: vs.map((v, i) => ({ t: String(i * 1_000_000), v })),
})

describe('hostStats', () => {
  it('latestValue takes the last non-null point', () => {
    expect(latestValue(s({}, [0.1, 0.5, null]))).toBe(0.5)
    expect(latestValue(s({}, [null, null]))).toBeNull()
    expect(latestValue(undefined)).toBeNull()
  })
  it('latestTotal sums latest values across series (net rx+tx)', () => {
    expect(latestTotal([s({ direction: 'receive' }, [100]), s({ direction: 'transmit' }, [40])])).toBe(140)
    expect(latestTotal([])).toBeNull()
  })
  it('worstSeries picks the max latest value and its label', () => {
    const disk = [s({ mountpoint: '/' }, [0.67]), s({ mountpoint: '/boot/efi' }, [0.04])]
    expect(worstSeries(disk, 'mountpoint')).toEqual({ label: '/', value: 0.67 })
    expect(worstSeries([], 'mountpoint')).toBeNull()
  })
  it('utilAccent thresholds at 0.8 warning / 0.9 error', () => {
    expect(utilAccent(0.5)).toBeUndefined()
    expect(utilAccent(0.8)).toBe('warning')
    expect(utilAccent(0.95)).toBe('error')
    expect(utilAccent(null)).toBeUndefined()
  })
  it('sparkValues strips nulls in order', () => {
    expect(sparkValues(s({}, [0.1, null, 0.3]))).toEqual([0.1, 0.3])
  })
  it('cpuSeriesForMode filters on the cpu label', () => {
    const cpu = [s({ cpu: 'total' }, [0.2]), s({ cpu: '0' }, [0.4]), s({ cpu: '1' }, [0.1])]
    expect(cpuSeriesForMode(cpu, 'total')).toHaveLength(1)
    expect(cpuSeriesForMode(cpu, 'per-core')).toHaveLength(2)
    expect(cpuSeriesForMode(undefined, 'total')).toEqual([])
  })
  it('tolerates a null label value (e.g. a not-yet-promoted attribute)', () => {
    const disk = [s({ mountpoint: '/' }, [0.2]), s({ mountpoint: null }, [0.9])]
    expect(worstSeries(disk, 'mountpoint')).toEqual({ label: '', value: 0.9 })
    const cpu = [s({ cpu: 'total' }, [0.2]), s({ cpu: null }, [0.4])]
    expect(cpuSeriesForMode(cpu, 'per-core')).toHaveLength(1)
    expect(cpuSeriesForMode(cpu, 'total')).toHaveLength(1)
  })
  it('formatPct renders a 0–1 fraction', () => {
    expect(formatPct(0.484)).toBe('48%')
    expect(formatPct(0.031)).toBe('3.1%')
    expect(formatPct(null)).toBe('—')
  })
  it('hostStatus takes the worst of the given utilizations through utilAccent thresholds', () => {
    expect(hostStatus([0.85])).toBe('warning')
    expect(hostStatus([0.5, 0.95])).toBe('error')
    expect(hostStatus([0.1, null, undefined])).toBeUndefined()
    expect(hostStatus([])).toBeUndefined()
    // error trumps warning regardless of order
    expect(hostStatus([0.95, 0.85])).toBe('error')
    expect(hostStatus([0.85, 0.95])).toBe('error')
  })
})
