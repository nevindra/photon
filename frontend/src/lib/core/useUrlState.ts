// URL <-> filter-state sync for the logs view. Query keys are short so the URL stays
// shareable: `svc` / `sev` (comma-joined lists), `q` (free-text search).
// `range`/`from`/`to`/`scope` are owned solely by context.ts and are preserved as-is —
// this module never reads or deletes them.
//
// `parseQuery` / `buildQuery` are pure and round-trip stable — the same object comes back
// out of `parseQuery(buildQuery(state))` (modulo array order/dedupe, which callers control).
import { ref, watch, type Ref } from 'vue'

// The per-view filter state this module owns (svc/sev/q). `timeRange` here is the
// legacy/local `range` key — distinct from context.ts's global time range.
export interface UrlFilterState {
  services: string[]
  severities: string[]
  timeRange: string | null
  text: string
}

// Refs a caller may hand to `useUrlState`; every key is optional (callers typically only
// wire up the ones they own, e.g. just `{ text }`).
export interface UrlStateRefs {
  services?: Ref<string[]>
  severities?: Ref<string[]>
  timeRange?: Ref<string | null>
  text?: Ref<string>
}

const DEFAULTS: UrlFilterState = Object.freeze({
  services: [],
  severities: [],
  timeRange: null,
  text: '',
})

function splitList(value: string | null | undefined): string[] {
  if (!value) return []
  return value
    .split(',')
    .map((v) => v.trim())
    .filter(Boolean)
}

// location.search (or any "?a=b&c=d" / "a=b&c=d" string) -> filter state.
// Missing/empty keys fall back to DEFAULTS.
export function parseQuery(search?: string | null): UrlFilterState {
  const params = new URLSearchParams(search ?? '')
  return {
    services: splitList(params.get('svc')),
    severities: splitList(params.get('sev')),
    timeRange: params.get('range') || null,
    text: params.get('q') || '',
  }
}

// Filter state -> query string. Empty/default values are omitted entirely so the URL
// stays clean. Returns '' when every field is empty (never a bare '?').
export function buildQuery({ services, severities, timeRange, text }: Partial<UrlFilterState> = {}): string {
  const params = new URLSearchParams()
  if (services && services.length) params.set('svc', services.join(','))
  if (severities && severities.length) params.set('sev', severities.join(','))
  if (timeRange) params.set('range', timeRange)
  if (text) params.set('q', text)
  const qs = params.toString()
  return qs ? `?${qs}` : ''
}

// Vue composable: seeds `refs` (services/severities/timeRange/text) from location.search
// on setup, then keeps location.search in sync (via history.replaceState, no navigation/
// history entries) whenever the refs change. SSR-safe: no-ops when `window` is undefined.
export function useUrlState(refs: UrlStateRefs): void {
  if (typeof window === 'undefined') return

  const initial = parseQuery(window.location.search)
  if (refs.services) refs.services.value = initial.services
  if (refs.severities) refs.severities.value = initial.severities
  if (refs.timeRange) refs.timeRange.value = initial.timeRange
  if (refs.text) refs.text.value = initial.text

  const OWNED = ['svc', 'sev', 'q']
  const sync = () => {
    const p = new URLSearchParams(window.location.search) // preserve range/from/to/scope (owned by context.ts)
    OWNED.forEach((k) => p.delete(k))
    const built = new URLSearchParams(
      buildQuery({
        services: refs.services?.value ?? DEFAULTS.services,
        severities: refs.severities?.value ?? DEFAULTS.severities,
        timeRange: refs.timeRange?.value ?? DEFAULTS.timeRange,
        text: refs.text?.value ?? DEFAULTS.text,
      }).replace(/^\?/, ''),
    )
    built.forEach((v, k) => p.set(k, v))
    const qs = p.toString()
    window.history.replaceState(null, '', qs ? `?${qs}` : window.location.pathname)
  }

  watch(
    [
      refs.services ?? ref(DEFAULTS.services),
      refs.severities ?? ref(DEFAULTS.severities),
      refs.timeRange ?? ref(DEFAULTS.timeRange),
      refs.text ?? ref(DEFAULTS.text),
    ],
    sync,
    { deep: true },
  )
}
