// Logical-pageview lifecycle. A "view" is decoupled from the document load: it rotates on every
// real client-side navigation (driven by spa.ts), so SPA routes each become their own pageview.
export type NavKind = "hard" | "soft";

export interface ViewDescriptor {
  id: string;
  route: string;
  path: string;
  traceId?: string;
  navStart: number;
  seq: number;
  prevRoute?: string;
  nav: NavKind;
}

const rid = () => (crypto.randomUUID?.() ?? String(Math.random()).slice(2) + Date.now().toString(36));
function perfNow() { return typeof performance !== "undefined" ? performance.timeOrigin + performance.now() : Date.now(); }

let current: ViewDescriptor = { id: rid(), route: "/", path: "/", navStart: perfNow(), seq: 0, nav: "hard" };
const listeners: Array<(ended: ViewDescriptor, started: ViewDescriptor) => void> = [];

export function initView(route: string, path: string): ViewDescriptor {
  current = { id: rid(), route, path, navStart: perfNow(), seq: 0, nav: "hard" };
  return current;
}

export function rotateView(route: string, path: string): { ended: ViewDescriptor; started: ViewDescriptor } {
  const ended = current;
  const started: ViewDescriptor = {
    id: rid(), route, path, navStart: perfNow(),
    seq: ended.seq + 1, prevRoute: ended.route, nav: "soft",
  };
  current = started;
  for (const cb of listeners) { try { cb(ended, started); } catch { /* never throw */ } }
  return { ended, started };
}

export function currentView(): ViewDescriptor { return current; }

export function endedDuration(d: ViewDescriptor, at: number = perfNow()): number {
  return Math.max(0, Math.round(at - d.navStart));
}

export function onViewChange(cb: (ended: ViewDescriptor, started: ViewDescriptor) => void): void {
  listeners.push(cb);
}
