import { describe, it, expect } from 'vitest'
import { api } from '@/lib/core/api'

describe('services api mock fallback', () => {
  it('serviceSettings falls back to a default-shaped object offline', async () => {
    const s = await api.serviceSettings('checkout')
    expect(s).toHaveProperty('apdex_threshold_ms')
    expect(s).toHaveProperty('is_default')
  })
  it('serviceTimeseries returns an array offline', async () => {
    const rows = await api.serviceTimeseries('web', { start: '0', end: '100', buckets: 2 })
    expect(Array.isArray(rows)).toBe(true)
  })
  it('serviceDependencies falls back to empty database/external groups offline', async () => {
    const deps = await api.serviceDependencies('checkout', { start: '0', end: '100' })
    expect(deps).toEqual({ database: [], external: [] })
  })
  it('setServiceSettings mocks a non-default response offline', async () => {
    const s = await api.setServiceSettings('checkout', 750)
    expect(s).toEqual({ apdex_threshold_ms: 750, is_default: false })
  })
  it('resetServiceSettings mocks the default response offline', async () => {
    const s = await api.resetServiceSettings('checkout')
    expect(s).toEqual({ apdex_threshold_ms: 500, is_default: true })
  })
})
