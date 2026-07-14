import { ref, type Ref } from 'vue'
import { api } from '@/lib/core/api'

// Server-reported boot/session state (`GET /api/session` and the `setup`/`login` success shapes
// all agree on this field set). `api.ts` is untyped JS for now, so these fields are asserted here
// rather than inferred.
export interface SessionInfo {
  authenticated: boolean
  username: string | null
  needs_setup: boolean
}

// Result of a login/setup attempt. `error` is only ever present alongside `ok: false`.
export interface AuthResult {
  ok: boolean
  error?: string
}

// App-wide auth state. A fresh page load starts unknown; `hydrate()` asks the server (the
// session cookie is httpOnly, so JS can't read it) and fills these in. Kept tiny and Pinia-free,
// matching the project's lean ethos.
export const authed: Ref<boolean> = ref(false)
export const username: Ref<string | null> = ref(null)
export const needsSetup: Ref<boolean> = ref(false)

// One-shot boot probe. Cached so the router guard can `await hydrate()` on every navigation but
// only hit the network once. Reset on logout so the next boot re-probes.
let hydration: Promise<SessionInfo> | null = null

export function hydrate(): Promise<SessionInfo> {
  if (!hydration) {
    hydration = (api.session() as Promise<SessionInfo>)
      .then((s) => {
        authed.value = !!s.authenticated
        username.value = s.username ?? null
        needsSetup.value = !!s.needs_setup
        return s
      })
      .catch(() => {
        authed.value = false
        username.value = null
        needsSetup.value = false
        return { authenticated: false, username: null, needs_setup: false }
      })
  }
  return hydration
}

export async function login(u: string, p: string): Promise<AuthResult> {
  const res: AuthResult = await api.login(u, p)
  if (res.ok) {
    authed.value = true
    username.value = u
    needsSetup.value = false
  }
  return res
}

export async function setup(u: string, p: string): Promise<AuthResult> {
  const res: AuthResult = await api.setup(u, p)
  if (res.ok) {
    authed.value = true
    username.value = u
    needsSetup.value = false
  }
  return res
}

export async function logout(): Promise<void> {
  await api.logout()
  authed.value = false
  username.value = null
  hydration = null // force a fresh probe on the next boot
}

// Test-only: seed a resolved hydration so the router guard doesn't hit the network and clobber a
// manually-set `authed`. `.reset()` clears the cache so a test can exercise the real probe.
// Typed as an intersection so the `.reset` static can be attached below — same pattern as the
// original JS (function + property assignment), just annotated.
type HydratedForTest = ((state?: SessionInfo) => void) & { reset: () => void }

const _setHydratedForTestImpl = (
  state: SessionInfo = { authenticated: false, needs_setup: false, username: null },
): void => {
  authed.value = !!state.authenticated
  needsSetup.value = !!state.needs_setup
  username.value = state.username ?? null
  hydration = Promise.resolve(state)
}

export const _setHydratedForTest = _setHydratedForTestImpl as HydratedForTest
_setHydratedForTest.reset = (): void => {
  hydration = null
}
