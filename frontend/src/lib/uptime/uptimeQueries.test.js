import { describe, it, expect } from 'vitest'
import { monitorsQueryKey, heartbeatsQueryKey } from '@/lib/uptime/uptimeQueries'

describe('uptime query keys', () => {
  it('are stable and parameterized', () => {
    expect(monitorsQueryKey()).toEqual(['monitors'])
    expect(heartbeatsQueryKey('abc', '7d')).toEqual(['heartbeats', 'abc', '7d'])
  })
})
