# @photon/rum

A tiny (**< 5 KB gzipped**) browser SDK that reports [Core Web Vitals](https://web.dev/vitals/)
and JavaScript errors to a [Photon](../../README.md) server. It wraps Google's `web-vitals`
library (correct INP/CLS windowing, LCP finalization, bfcache handling) and adds error capture,
session/view identity, context enrichment, batching, and the beacon itself — one call,
`initPhoton(opts)`, wires everything up.

Vitals are stored on Photon as gauge metrics (`web_vitals.lcp` / `.inp` / `.cls` / `.fcp` /
`.ttfb`); JS errors are stored as `ERROR` logs. No new storage signal, no new query surface — RUM
data shows up in the existing metrics/logs machinery, surfaced by a purpose-built `/rum` UI
section. See [`docs/subsystems/rum.md`](../../docs/subsystems/rum.md) for the full design.

## Install

Bundler apps (tree-shakeable ESM):

```bash
bun add @photon/rum
```

```ts
import { initPhoton } from "@photon/rum";

initPhoton({
  app: "web-storefront",
  endpoint: "https://photon.example.com",
  key: "pk_live_…",
});
```

No-bundler / plain-HTML apps — drop in the IIFE build, which exposes a global `PhotonRUM`:

```html
<script src="https://your-cdn/photon-rum.iife.js"></script>
<script>
  PhotonRUM.initPhoton({
    app: "web-storefront",
    endpoint: "https://photon.example.com",
    key: "pk_live_…",
  });
</script>
```

Load it `async`/`defer` — the SDK never blocks the critical path and never throws into the host
app (all capture is wrapped; SDK-internal errors are silently dropped).

## `initPhoton(opts)`

```ts
interface PhotonOptions {
  app: string;                              // -> service.name on every row from this app
  endpoint: string;                         // Photon server base URL (beacon posts to `${endpoint}/api/rum`)
  key: string;                              // the app's PUBLIC key (see "Registering an app" below)
  sampleRate?: number;                      // 0.0..1.0 client-side session sampling; default: report everything
  routeOf?: (path: string) => string;       // optional path -> route-pattern mapper (e.g. "/p/123" -> "/p/:id")
  attribution?: boolean;                    // opt-in LCP/INP/CLS sub-part breakdown (see below)
  tracing?: boolean;                        // opt-in W3C traceparent propagation on fetch/XHR (see below)
  tracePropagationTargets?: (string | RegExp)[]; // where to inject it; default ["same-origin"]
}
```

`initPhoton` is fire-and-forget: it wires up `web-vitals` listeners and `window`
error/`unhandledrejection` handlers, then buffers reports and flushes them via
`navigator.sendBeacon` (falling back to `fetch(..., { keepalive: true })`) on `visibilitychange` →
hidden and `pagehide` — not one request per metric.

### Attribution (opt-in)

Set `attribution: true` to also capture *why* a vital was slow: LCP's four sub-parts (TTFB,
resource load delay, resource load time, element render delay) plus the LCP element, INP's target
+ phase breakdown, and CLS's largest-shift source. This pulls in `web-vitals/attribution` via a
dynamic `import()`, so the base bundle stays tree-shaken and under budget when you don't opt in.
The `/rum` UI's page-detail view renders the LCP breakdown as a segmented bar when this data is
present.

### Trace correlation (opt-in)

Set `tracing: true` to correlate a pageview's Web Vitals and JS errors with your backend's trace
waterfalls:

```ts
initPhoton({
  app: "web-storefront",
  endpoint: "https://photon.example.com",
  key: "pk_live_…",
  tracing: true,
  tracePropagationTargets: ["same-origin", "https://api.example.com", /\/graphql$/],
});
```

On init, the SDK mints one W3C trace id for the pageview (32 lowercase hex chars via
`crypto.getRandomValues`; the module silently no-ops if `crypto` is unavailable) and patches
`window.fetch` / `XMLHttpRequest` to inject a fresh `traceparent: 00-<trace-id>-<span-id>-01` header
on each outgoing request that matches `tracePropagationTargets`:

- Default is `["same-origin"]` — only requests to the page's own origin get the header, so
  `tracing: true` is CORS-safe out of the box with no extra config.
- A `string` entry is an exact-origin match (or the literal `"same-origin"`); a `RegExp` is tested
  against the full resolved request URL. Origins that don't match are **never** touched.
- Once initialized, every beacon from that pageview — vitals *and* errors — also carries the trace
  id as a `trace` field. The server validates/normalizes it (exactly 32 hex digits, non-zero) before
  writing it to the log's **native** `trace_id` column (no schema change); malformed or missing
  values are silently dropped, and beacons from older SDK versions simply omit the field. This is
  what the `/rum` error-issue detail view's "Open trace" links depend on — it's populated only for
  errors ingested after `tracing` is turned on.
- Like the rest of the SDK, this never throws into the host app: any internal failure leaves
  `fetch`/`XHR` in their normal, un-instrumented state.
- Lazily loaded: `tracing: true` triggers a dynamic `import("./tracing")`, so it costs nothing
  against the < 5 KB core budget unless you opt in.

## Registering an app (server side)

Every beacon must name an app that's registered in the Photon server's config — the SDK's `key`
is checked against it, and `app` becomes the row's `service.name` (server-derived, never trusted
from the beacon body):

```toml
# photon.toml
[[rum.apps]]
name = "web-storefront"                              # matches `app` above
key = "pk_live_…"                                     # matches `key` above — PUBLIC, safe to ship in client JS
allowed_origins = ["https://shop.example.com"]        # CORS allowlist: the actual browser auth boundary
sample_rate = 1.0                                      # optional server-side cap, independent of the client sampleRate
rate_limit  = 5000                                     # optional beacons/sec/app cap
```

`key` is a **public** identifier, not a secret — it only names the app to the beacon handler and
cannot read data or authenticate any other endpoint. `allowed_origins` is what actually gates who
can post: `POST /api/rum` is the only unauthenticated, CORS-enabled route in Photon (there's no
session cookie to hold from a browser beacon), so the server checks the request's `Origin` header
against this list. See [`photon.example.toml`](../../photon.example.toml) for the full commented
example, including validation rules (non-empty `name`/`key`/`allowed_origins`, unique `key`,
`sample_rate` in `0.0..=1.0`).

## Works over plain HTTP

The Web Vitals APIs this SDK relies on (`PerformanceObserver`, Navigation/Paint/Layout-Shift
timing) are available on plain `http://` origins — HTTPS is not required to collect Core Web
Vitals. (It *is* required for `navigator.sendBeacon` on some older browsers to reach a
cross-origin HTTPS endpoint, but same-scheme HTTP → HTTP beaconing works fine; the SDK also falls
back to `fetch(..., { keepalive: true })` if `sendBeacon` is unavailable or rejects the payload.)

## Development

```bash
bun install
bun run build   # tsup -> dist/photon-rum.js (ESM) + dist/photon-rum.iife.js (global PhotonRUM)
bun run test    # vitest (jsdom)
bun run size    # gzip the ESM build and fail if it exceeds the 5 KB budget (scripts/size-check.mjs)
```

**Layout:** `src/index.ts` (`initPhoton` + vitals/error wiring), `src/session.ts` (in-memory
session + view id, 30 min idle / 4 h max rotation), `src/context.ts` (UA + connection type),
`src/errors.ts` (capture + dedup + client-side rate-limit), `src/beacon.ts` (buffer + flush),
`src/attribution.ts` (tree-shakeable, dynamically imported only when `attribution: true`),
`src/trace.ts` (pure trace-id generation + `tracePropagationTargets` matching), `src/traceState.ts`
(a tiny shared holder so `index.ts` can read the pageview trace id without a static import),
`src/tracing.ts` (the fetch/XHR patching — tree-shakeable, dynamically imported only when
`tracing: true`).
