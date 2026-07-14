import { describe, it, expect } from 'vitest'
import { serviceColorClass, SERVICE_PALETTE } from '@/lib/services/serviceColor'

describe('serviceColorClass', () => {
  it('is deterministic for a given name', () => {
    expect(serviceColorClass('checkout')).toBe(serviceColorClass('checkout'))
  })
  it('returns a class from the fixed palette', () => {
    expect(SERVICE_PALETTE).toContain(serviceColorClass('api'))
  })
  it('handles empty/undefined names without throwing', () => {
    expect(SERVICE_PALETTE).toContain(serviceColorClass(''))
    expect(SERVICE_PALETTE).toContain(serviceColorClass(undefined))
  })
})
