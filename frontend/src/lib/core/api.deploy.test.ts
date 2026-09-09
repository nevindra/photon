// The availability switch, proven outside a dev build: a backend that answers the deploy routes
// with an EMPTY 404 (no subsystem) must NOT be mocked over unless demo mode is on.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { api } from '@/lib/core/api'
import { isDeployUnavailable, DEPLOY_DEMO_KEY } from '@/lib/deploy/availability'

function respond(status: number, body = '', headers: Record<string, string> = {}) {
  return vi.fn(async () => new Response(body, { status, headers }))
}

// ky builds `new Request('/api/…')`, which Node's Request rejects as a relative URL BEFORE fetch
// runs — that is why the other api tests see "network down". Resolve it against an origin so the
// stubbed fetch actually answers.
const RealRequest = globalThis.Request
class AbsoluteRequest extends RealRequest {
  constructor(input: RequestInfo | URL, init?: RequestInit) {
    super(typeof input === 'string' && input.startsWith('/') ? `http://localhost${input}` : input, init)
  }
}

describe('deploy availability gate', () => {
  const realFetch = globalThis.fetch
  beforeEach(() => {
    globalThis.Request = AbsoluteRequest as unknown as typeof Request
    localStorage.removeItem(DEPLOY_DEMO_KEY)
    vi.stubEnv('DEV', false)
    api.mock = false
  })
  afterEach(() => {
    globalThis.Request = RealRequest
    globalThis.fetch = realFetch
    vi.unstubAllEnvs()
  })

  it('an empty 404 outside demo mode is "not enabled", not demo data', async () => {
    globalThis.fetch = respond(404) as unknown as typeof fetch
    await expect(api.stacks()).rejects.toSatisfy((e: unknown) => isDeployUnavailable(e))
    expect(api.mock).toBe(false)
    const res = await api.createStack({ name: 'x', host: 'h', files: [], deploy_options: { pull: 'missing', force_recreate: false, recreate_volumes: false }, acknowledged_warnings: [] })
    expect(res.ok).toBe(false)
    expect(res.error).toContain('not enabled')
  })

  it('the demo flag opts into the mock corpus', async () => {
    localStorage.setItem(DEPLOY_DEMO_KEY, '1')
    globalThis.fetch = respond(404) as unknown as typeof fetch
    const res = await api.stacks()
    expect(res.stacks.length).toBeGreaterThan(0)
    expect(api.mock).toBe(true)
  })

  it('a JSON-bodied 404 is a real answer from a backend that has the subsystem', async () => {
    globalThis.fetch = respond(404, JSON.stringify({ error: 'no such stack' }), { 'content-type': 'application/json' }) as unknown as typeof fetch
    await expect(api.stack('nope')).resolves.toBeNull()
    expect(api.mock).toBe(false)
  })

  it('a 500 is a real failure, never demo data', async () => {
    globalThis.fetch = respond(500, 'boom') as unknown as typeof fetch
    await expect(api.stacks()).rejects.toBeTruthy()
    expect(api.mock).toBe(false)
  })
})
