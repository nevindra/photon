import { describe, it, expect, beforeEach } from 'vitest'
import { router } from '@/router/index.js'
import { authed, needsSetup, _setHydratedForTest } from '@/lib/core/auth'

// The guard is a function of hydrated auth state + the target route (see router/index.test.js).
// Pre-seed hydration so the guard's `await hydrate()` resolves instantly without a network probe.
describe('router / -> /home', () => {
  beforeEach(async () => {
    _setHydratedForTest({ authenticated: true, needs_setup: false, username: null })
    authed.value = true
    needsSetup.value = false
    await router.replace('/logs')
    await router.isReady()
  })

  it('/ redirects to /home', async () => {
    await router.push('/')
    await router.isReady()
    expect(router.currentRoute.value.path).toBe('/home')
  })

  it('/home resolves to the home route', async () => {
    await router.push('/home')
    expect(router.currentRoute.value.name).toBe('home')
  })
})
