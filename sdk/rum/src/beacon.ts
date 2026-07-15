import { type ViewDescriptor, endedDuration } from "./view";

function viewObject(d: ViewDescriptor, includeDur: boolean) {
  const v: any = { id: d.id, route: d.route, path: d.path, seq: d.seq, nav: d.nav };
  // `dur` (time-on-view) is emitted only when the view is finalized (flush), not on a supplemental
  // late-vital beacon (sendImmediate) — else the view's view_duration would be sent more than once.
  if (includeDur) v.dur = endedDuration(d);
  if (d.prevRoute) v.prev = d.prevRoute;
  return v;
}

export interface Beacon {
  vital: (v: any) => void;
  error: (e: any) => void;
  flush: (descriptor: ViewDescriptor) => void;
  sendImmediate: (descriptor: ViewDescriptor, payload: { vitals?: any[]; errors?: any[] }) => void;
}

export function makeBeacon(endpoint: string, staticBase: () => object): Beacon {
  const url = endpoint.replace(/\/$/, "") + "/api/rum";
  let vitals: any[] = [], errors: any[] = [];

  const post = (body: string) => {
    try {
      const blob = new Blob([body], { type: "text/plain" });
      if (!navigator.sendBeacon?.(url, blob)) {
        fetch(url, { method: "POST", body, keepalive: true, headers: { "content-type": "text/plain" } }).catch(() => {});
      }
    } catch { /* swallow */ }
  };

  const send = (descriptor: ViewDescriptor, vs: any[], es: any[], includeDur: boolean) => {
    if (!vs.length && !es.length) return;
    try {
      const body: any = { ...staticBase(), view: viewObject(descriptor, includeDur), vitals: vs, errors: es };
      if (descriptor.traceId) body.trace = descriptor.traceId;
      post(JSON.stringify(body));
    } catch { /* swallow */ }
  };

  const flush = (descriptor: ViewDescriptor) => {
    if (!vitals.length && !errors.length) return;
    const vs = vitals, es = errors;
    vitals = []; errors = [];
    send(descriptor, vs, es, true);   // a flush finalizes the view → carries view_duration
  };

  const sendImmediate = (descriptor: ViewDescriptor, payload: { vitals?: any[]; errors?: any[] }) =>
    send(descriptor, payload.vitals ?? [], payload.errors ?? [], false);   // supplemental → no dur

  return { vital: (v) => vitals.push(v), error: (e) => errors.push(e), flush, sendImmediate };
}
