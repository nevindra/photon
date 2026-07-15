// Opt-in: patch fetch + XHR to inject a W3C `traceparent` on same-origin (or configured) requests,
// so each backend entry span parents to the pageview trace. Wrapped so it never throws — a failure
// restores/bypasses cleanly and the host app's requests are untouched.
import { pageTraceId, newSpanId, traceparent, matchesTarget, bindTraceToViews } from "./trace";

export interface TracingOptions {
  tracePropagationTargets?: (string | RegExp)[];
}

export function initTracing(opts: TracingOptions = {}): void {
  const id = pageTraceId();
  if (!id) return; // crypto unavailable → no-op
  bindTraceToViews();          // ensures the current view has a trace id, and rotates it per view
  patchFetch(opts.tracePropagationTargets);
  patchXhr(opts.tracePropagationTargets);
}

/** A fresh per-request traceparent sharing the one pageview trace id. */
function header(): string {
  return traceparent(pageTraceId(), newSpanId());
}

function patchFetch(targets?: (string | RegExp)[]): void {
  if (typeof window === "undefined" || typeof window.fetch !== "function") return;
  const orig = window.fetch;
  window.fetch = function (input: RequestInfo | URL, init?: RequestInit) {
    try {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.href : (input as Request).url;
      if (matchesTarget(url, targets)) {
        const h = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
        if (!h.has("traceparent")) h.set("traceparent", header());
        init = { ...init, headers: h };
      }
    } catch {
      /* leave the request untouched */
    }
    try {
      return orig.call(this, input as any, init);
    } catch (err) {
      // Real fetch never throws synchronously (it rejects) — preserve that contract even if the
      // underlying implementation misbehaves, so callers can rely on `.catch()`.
      return Promise.reject(err);
    }
  };
}

function patchXhr(targets?: (string | RegExp)[]): void {
  if (typeof XMLHttpRequest === "undefined") return;
  const proto = XMLHttpRequest.prototype as any;
  const origOpen = proto.open;
  const origSend = proto.send;
  proto.open = function (method: string, url: string, ...rest: any[]) {
    try {
      this.__ptTrace = matchesTarget(url, targets);
    } catch {
      this.__ptTrace = false;
    }
    return origOpen.call(this, method, url, ...rest);
  };
  proto.send = function (body?: any) {
    try {
      if (this.__ptTrace) this.setRequestHeader("traceparent", header());
    } catch {
      /* ignore */
    }
    return origSend.call(this, body);
  };
}
