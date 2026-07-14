import { describe, it, expect, beforeEach, vi } from 'vitest'
import { authed, username, needsSetup, hydrate, _setHydratedForTest } from '@/lib/core/auth'
import { api } from '@/lib/core/api'

describe('auth.hydrate', () => {
  beforeEach(() => {
    _setHydratedForTest({ authenticated: false, needs_setup: false, username: null })
    // Reset the one-shot cache so each test re-probes.
    _setHydratedForTest.reset()
  })

  it('reflects an authenticated session', async () => {
    vi.spyOn(api, 'session').mockResolvedValue({
      authenticated: true,
      username: 'alice',
      needs_setup: false,
    })
    await hydrate()
    expect(authed.value).toBe(true)
    expect(username.value).toBe('alice')
    expect(needsSetup.value).toBe(false)
  })

  it('reflects a needs-setup instance', async () => {
    vi.spyOn(api, 'session').mockResolvedValue({
      authenticated: false,
      username: null,
      needs_setup: true,
    })
    await hydrate()
    expect(authed.value).toBe(false)
    expect(needsSetup.value).toBe(true)
  })
})
