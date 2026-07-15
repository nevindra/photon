// Pageview-scoped W3C trace context. Pure, feature-detected, never throws. Reachable only via the
// opt-in tracing chunk (never index.ts), so id-gen stays off the measured core bundle.

import { currentView, onViewChange } from "./view";

const hasCrypto = typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function";

/** `bytes` random bytes as lowercase hex, or "" if crypto is unavailable (tracing then no-ops). */
function randomHex(bytes: number): string {
  if (!hasCrypto) return "";
  const b = new Uint8Array(bytes);
  crypto.getRandomValues(b);
  let s = "";
  for (let i = 0; i < b.length; i++) s += (b[i] ?? 0).toString(16).padStart(2, "0");
  return s;
}

/** 16-byte / 32-hex W3C trace id. */
export function newTraceId(): string {
  return randomHex(16);
}

/** 8-byte / 16-hex W3C span/parent id. */
export function newSpanId(): string {
  return randomHex(8);
}

/** W3C `traceparent`: version-00, sampled-01. */
export function traceparent(traceId: string, spanId: string): string {
  return `00-${traceId}-${spanId}-01`;
}

// Current pageview trace id: minted per view and cached on the view descriptor.
export function pageTraceId(): string {
  const v = currentView();
  if (!v.traceId) v.traceId = newTraceId();
  return v.traceId;
}

// Subscribe so every view rotation mints a fresh trace id, cached on the new view descriptor. The
// beacon reads it via `descriptor.traceId` and the fetch/XHR header via `pageTraceId()` (both off the
// current view), so there is no separate published copy to keep in sync. Call once from initTracing.
export function bindTraceToViews(): void {
  pageTraceId();   // ensure the current (landing) view has a trace id
  onViewChange((_ended, started) => {
    try {
      if (!started.traceId) started.traceId = newTraceId();
    } catch { /* never throw */ }
  });
}

/**
 * True if `url` should receive the traceparent header, per `tracePropagationTargets`:
 * - "same-origin" (also the default when targets is empty) → same origin as the current page
 * - any other string → exact origin match (e.g. "https://api.example.com")
 * - RegExp → tested against the full resolved URL
 */
export function matchesTarget(url: string, targets?: (string | RegExp)[]): boolean {
  let u: URL;
  try {
    u = new URL(url, location.href);
  } catch {
    return false;
  }
  const list = targets && targets.length ? targets : ["same-origin"];
  for (const t of list) {
    if (typeof t === "string") {
      if (t === "same-origin" ? u.origin === location.origin : u.origin === t) return true;
    } else if (t.test(u.href)) {
      return true;
    }
  }
  return false;
}
