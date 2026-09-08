// Whether this Photon has the Docker fleet-management backend at all, and the explicit demo
// switch that lets the screens run over the mock corpus without one. Kept apart from `api.ts` so
// views and tests can import it without pulling the whole client (test files mock `api` wholesale).
export const DEPLOY_DEMO_KEY = 'photon-deploy-demo'
export const DEPLOY_UNAVAILABLE_MESSAGE = 'Docker fleet management is not enabled on this server'

export class DeployUnavailableError extends Error {
  constructor() {
    super(DEPLOY_UNAVAILABLE_MESSAGE)
    this.name = 'DeployUnavailableError'
  }
}

export function isDeployUnavailable(e: unknown): e is DeployUnavailableError {
  return e instanceof DeployUnavailableError || (e instanceof Error && e.name === 'DeployUnavailableError')
}

/** The explicit opt-in: a dev build, or `localStorage['photon-deploy-demo'] = '1'`. */
export function deployDemoFlag(): boolean {
  try {
    if (typeof localStorage !== 'undefined' && localStorage.getItem(DEPLOY_DEMO_KEY) === '1') return true
  } catch {
    /* storage blocked — fall through */
  }
  return !!import.meta.env.DEV
}

export function setDeployDemo(on: boolean): void {
  try {
    if (on) localStorage.setItem(DEPLOY_DEMO_KEY, '1')
    else localStorage.removeItem(DEPLOY_DEMO_KEY)
  } catch {
    /* storage blocked */
  }
}
