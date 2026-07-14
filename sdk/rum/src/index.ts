import { onLCP, onINP, onCLS, onFCP, onTTFB } from "web-vitals";
import { sessionId, viewId } from "./session";
import { context, view } from "./context";
import { collectErrors } from "./errors";
import { makeBeacon } from "./beacon";
import { traceState } from "./traceState";

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
   *  error's beacon with the pageview trace id. Loaded on demand (dynamic import) like `attribution`. */
  tracing?: boolean;
  /** Where to inject `traceparent`. Default `["same-origin"]`. Strings are exact origins (or the
   *  literal "same-origin"); RegExps test the full URL. Off-list origins are never touched (CORS-safe). */
  tracePropagationTargets?: (string | RegExp)[];
}

export function initPhoton(opts: PhotonOptions): void {
  try {
    if (typeof window === "undefined") return;
    if (opts.sampleRate != null && Math.random() > opts.sampleRate) return;
    const base = () => ({
      app: opts.app,
      key: opts.key,
      session: sessionId(),
      view: { id: viewId, ...view(opts.routeOf) },
      ctx: context(),
      ...(traceState.id ? { trace: traceState.id } : {}),
    });
    const b = makeBeacon(opts.endpoint, base);
    const report = (name: string) => (m: { value: number }) => b.vital({ n: name, v: name === "CLS" ? m.value : Math.round(m.value) });
    if (opts.attribution) {
      import("./attribution")
        .then((m) => m.initAttribution(b))
        .catch(() => { onLCP(report("LCP")); onINP(report("INP")); onCLS(report("CLS")); });
    } else {
      onLCP(report("LCP")); onINP(report("INP")); onCLS(report("CLS"));
    }
    onFCP(report("FCP")); onTTFB(report("TTFB"));
    if (opts.tracing) {
      import("./tracing")
        .then((m) => m.initTracing({ tracePropagationTargets: opts.tracePropagationTargets }))
        .catch(() => {});
    }
    collectErrors((e) => b.error(e));
  } catch { /* SDK must never break the host app */ }
}
