import { describe, it, expect, beforeEach } from 'vitest'
import { router } from './index.js'
import { authed, needsSetup, _setHydratedForTest } from '@/lib/core/auth'

// The guard is a function of hydrated auth state + the target route. We pre-seed hydration so the
// guard's `await hydrate()` resolves instantly without a network probe, then drive `authed`.
describe('router auth guard', () => {
  beforeEach(async () => {
    _setHydratedForTest({ authenticated: false, needs_setup: false, username: null })
    authed.value = false
    needsSetup.value = false
    await router.replace('/login')
    await router.isReady()
  })

  it('redirects an unauthenticated visit to a protected route to /login with a redirect back', async () => {
    authed.value = false
    await router.push('/traces')
    expect(router.currentRoute.value.path).toBe('/login')
    expect(router.currentRoute.value.query.redirect).toBe('/traces')
  })

  it('allows a protected route once authenticated', async () => {
    authed.value = true
    await router.push('/traces')
    expect(router.currentRoute.value.path).toBe('/traces')
  })

  it('bounces an authenticated user off /login to the redirect target', async () => {
    authed.value = true
    await router.push('/login?redirect=/traces')
    expect(router.currentRoute.value.path).toBe('/traces')
  })

  it('carries a deep-link (query included) into the redirect', async () => {
    authed.value = false
    await router.push('/traces/abc?t=42')
    expect(router.currentRoute.value.path).toBe('/login')
    expect(router.currentRoute.value.query.redirect).toBe('/traces/abc?t=42')
  })

  it('routes /metrics to the metrics explorer when authenticated', async () => {
    authed.value = true
    await router.push('/metrics')
    expect(router.currentRoute.value.name).toBe('metrics')
  })

  it('routes /services to the services list and /services/:id to detail', () => {
    const match = router.resolve('/services')
    expect(match.name).toBe('services')
    const detail = router.resolve('/services/checkout')
    expect(detail.name).toBe('service-detail')
    expect(detail.params.service).toBe('checkout')
  })

  it('redirects the old RED path to /services', async () => {
    authed.value = true
    await router.push('/traces/metrics')
    expect(router.currentRoute.value.path).toBe('/services')
    expect(router.currentRoute.value.name).toBe('services')
  })

  it('forces onboarding when the instance has no users', async () => {
    needsSetup.value = true
    await router.push('/logs')
    expect(router.currentRoute.value.path).toBe('/onboarding')
  })

  it('keeps onboarding off once a user exists', async () => {
    needsSetup.value = false
    authed.value = false
    await router.push('/onboarding')
    expect(router.currentRoute.value.path).toBe('/login')
  })
})
