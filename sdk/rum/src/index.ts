import { onLCP, onFCP, onTTFB } from "web-vitals";
import { sessionId } from "./session";
import { initView, currentView, onViewChange, type ViewDescriptor } from "./view";
import { context } from "./context";
import { collectErrors } from "./errors";
import { makeBeacon } from "./beacon";
import { initSpa, finalizeViewVitals, trackView } from "./spa";

export { trackView };

export interface PhotonOptions {
  app: string;
  endpoint: string;
  key: string;
  sampleRate?: number;
  routeOf?: (path: string) => string;
  /** Opt-in: LCP/INP/CLS sub-part breakdown via `web-vitals/attribution`, loaded on demand
   *  (dynamic import) so the base bundle stays tree-shaken. */
  attribution?: boolean;
  /** Opt-in: propagate a W3C `traceparent` on fetch/XHR to `tracePropagationTargets`, and tag each
   *  view's beacon with the per-view trace id. Loaded on demand (dynamic import) like `attribution`. */
  tracing?: boolean;
  /** Where to inject `traceparent`. Default `["same-origin"]`. Strings are exact origins (or the
   *  literal "same-origin"); RegExps test the full URL. Off-list origins are never touched (CORS-safe). */
  tracePropagationTargets?: (string | RegExp)[];
}

export function initPhoton(opts: PhotonOptions): void {
  try {
    if (typeof window === "undefined") return;
    if (opts.sampleRate != null && Math.random() > opts.sampleRate) return;

    const path = location.pathname;
    const route = opts.routeOf ? opts.routeOf(path) : path;
    const landing = initView(route, path);

    const staticBase = () => ({ app: opts.app, key: opts.key, session: sessionId(), ctx: context() });
    const b = makeBeacon(opts.endpoint, staticBase);

    // Buffer a vital for the current view. `route_change` is meaningful only for soft views.
    const reportVital = (m: { n: string; v: number }) => {
      if (m.n === "route_change" && currentView().nav === "hard") return;
      b.vital(m);
    };

    // Load-time web-vitals (LCP/FCP/TTFB) are pinned to the LANDING view — even if they finalize
    // after the user has soft-navigated away (sent as a follow-up beacon for `landing`). CLS and INP
    // are NOT taken from web-vitals: spa.ts measures them per view (one source of truth across hard +
    // soft views), which avoids double-counting and the page-lifetime mis-attribution web-vitals'
    // CLS/INP would give in a multi-route SPA.
    const hardReport = (name: string) => (m: { value: number }) => {
      const v = { n: name, v: Math.round(m.value) };
      if (currentView().id === landing.id) b.vital(v);
      else b.sendImmediate(landing, { vitals: [v] });
    };
    if (opts.attribution) {
      import("./attribution")
        .then((mod) => mod.initAttribution({ vital: (v: any) => {
          if (currentView().id === landing.id) b.vital(v); else b.sendImmediate(landing, { vitals: [v] });
        } }))
        .catch(() => { onLCP(hardReport("LCP")); });
    } else {
      onLCP(hardReport("LCP"));
    }
    onFCP(hardReport("FCP")); onTTFB(hardReport("TTFB"));

    if (opts.tracing) {
      import("./tracing")
        .then((mod) => mod.initTracing({ tracePropagationTargets: opts.tracePropagationTargets }))
        .catch(() => {});
    }

    collectErrors((e) => b.error(e));

    // On each real route change: finalize + flush the outgoing view, then the new view's observers
    // (already reset inside spa.ts) take over. onViewChange fires SYNCHRONOUSLY inside rotateView,
    // BEFORE spa.ts's startViewMeasurement resets the CLS/INP accumulators — so this captures the
    // ended view's per-view vitals. Do NOT move this into initSpa's onRouteChange (fires after reset).
    onViewChange((ended: ViewDescriptor) => {
      try { finalizeViewVitals(); } catch { /* ignore */ }
      b.flush(ended);
    });

    // Close out the final view when the tab hides / unloads.
    const flushCurrent = () => { try { finalizeViewVitals(); } catch {} b.flush(currentView()); };
    addEventListener("visibilitychange", () => { if (document.visibilityState === "hidden") flushCurrent(); });
    addEventListener("pagehide", flushCurrent);

    initSpa({ onRouteChange: () => {}, reportVital, routeOf: opts.routeOf });
  } catch { /* SDK must never break the host app */ }
}
