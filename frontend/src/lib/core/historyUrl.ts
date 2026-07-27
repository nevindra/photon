// The ONE way this app rewrites the URL's query string without navigating.
//
// Several layers merge-write their own keys into `location.search` outside of vue-router:
// context.ts (`range`/`from`/`to`/`scope`), useUrlState.ts (`svc`/`sev`/`q`), and per-view layering
// watchers (traces' `sort`/`mode`, metrics' `metric`/`agg`/`group`/`q`/`viz`, uptime's `q`). They
// all called `history.replaceState(null, …)` directly, which breaks vue-router in TWO ways — both
// of which had to be fixed for filters to survive a drill-in → back round trip:
//
//  1. `replaceState` REPLACES the entry's state object, and vue-router keeps its bookkeeping there
//     (`{ back, current, forward, position, scroll }`). Passing `null` wiped it. Since
//     `router.afterEach` calls `syncContextToUrl()` after EVERY navigation, the state was destroyed
//     the moment you landed anywhere — so `history.state.back` was gone and useBackTo.ts could
//     never recognise the list entry it came from (it fell back to a bare, filterless push).
//     vue-router warns about precisely this in dev: "history.state seems to have been manually
//     replaced without preserving the necessary values."
//
//  2. Less obvious, and the reason preserving the state alone still wasn't enough: vue-router
//     tracks the current entry's URL in `state.current`, and its `push()` re-asserts that value
//     onto the entry before pushing the new one —
//         changeLocation(currentState.current, currentState, /* replace */ true)
//     (see useHistoryStateNavigation in vue-router's dist). A stale `current` therefore REVERTS
//     the URL we just merged into: navigate away from `/traces?q=kind:server` and the entry
//     silently becomes `/traces` again, so going back restored a filterless page. Moving `current`
//     along with the URL is what actually keeps the filters on the entry.
//
// So: same state object, same `back`/`forward`/`position`/`scroll`, only `current` re-pointed at
// the URL we just wrote. `current` is base-relative, so it's derived from the existing value
// rather than from `location.pathname` — that stays correct under a non-'/' router base.
//
// SSR-safe: no-ops when `window` is undefined.

// Swap the query string on a location-ish string, keeping its path and hash.
function withSearch(location: string, qs: string, hash: string): string {
  const end = location.search(/[?#]/)
  const path = end === -1 ? location : location.slice(0, end)
  return path + (qs ? `?${qs}` : '') + hash
}

export function replaceSearch(search: string | URLSearchParams): void {
  if (typeof window === 'undefined') return
  const qs = (typeof search === 'string' ? search : search.toString()).replace(/^\?/, '')
  const { pathname, hash } = window.location
  const url = withSearch(pathname, qs, hash)

  const state = window.history.state
  const next =
    state && typeof state === 'object' && typeof (state as { current?: unknown }).current === 'string'
      ? { ...state, current: withSearch((state as { current: string }).current, qs, hash) }
      : state

  window.history.replaceState(next, '', url)
}
