// Back-to-list navigation that keeps the list's filter state.
//
// A detail view's back button used to `router.push('/traces')` — a brand-new history entry with a
// bare path, so every filter the explorer had encoded in its query string (`q`/`sort`/`mode`, plus
// context's `range`/`from`/`to`/`scope`) was dropped on the way back. The explorer persists its
// filters onto its OWN history entry (useUrlState + context.ts both write via
// `history.replaceState`), so the fix is to *return* to that entry rather than synthesize a new
// one: `router.back()` when the previous entry is the list we're going back to, `router.push` only
// as the deep-link fallback (arrived here from a log line, a shared URL, a fresh tab).
import type { Router } from 'vue-router'

// Pure: is `back` (vue-router's `history.state.back`, a full path incl. query) an entry on
// `listPath`? Matches the bare path and any query string on it, never a different route that
// merely shares the prefix (`/traces-foo`, `/traces/abc`).
export function isListEntry(back: unknown, listPath: string): boolean {
  if (typeof back !== 'string' || !back) return false
  const end = back.search(/[?#]/)
  return (end === -1 ? back : back.slice(0, end)) === listPath
}

// Navigate back to `listPath`, reusing the previous history entry (and therefore its filters)
// when that entry IS the list. SSR-safe: falls back to a push when there's no `window`.
export function backTo(router: Router, listPath: string): void {
  const back = typeof window === 'undefined' ? null : window.history.state?.back
  if (isListEntry(back, listPath)) router.back()
  else router.push(listPath)
}
