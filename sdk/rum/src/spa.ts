// Soft-navigation tracking for SPAs. Patches the History API (pushState/replaceState) and listens
// to popstate to detect *real* route changes, rotates the logical view, and measures per-view
// CLS/INP + a route_change render-time heuristic. Wrapped so it never throws into the host.
import { rotateView, currentView, type ViewDescriptor } from "./view";

export interface SpaHooks {
  onRouteChange: (ended: ViewDescriptor, started: ViewDescriptor) => void;
  reportVital: (v: { n: string; v: number }) => void;
  routeOf?: (path: string) => string;
}

let hooks: SpaHooks | null = null;
let lastKey = "";

function perfNow() { return typeof performance !== "undefined" ? performance.timeOrigin + performance.now() : Date.now(); }
function pathOf(): string { return location.pathname; }
function keyOf(path: string): string { return hooks?.routeOf ? hooks.routeOf(path) : path; }

function maybeRotate(): void {
  try {
    const path = pathOf();
    const key = keyOf(path);
    // Rotate only on a real route change. popstate (browser back/forward) reads the URL the browser
    // has ALREADY updated, so this same guard rotates on a genuine path change and — consistent with
    // pushState — skips a same-path back/forward (query/hash-only history entries).
    if (key === lastKey) return;      // query/hash-only change → same view
    lastKey = key;
    const { ended, started } = rotateView(key, path);
    startViewMeasurement(started);
    try { hooks?.onRouteChange(ended, started); } catch { /* never throw */ }
  } catch { /* never throw */ }
}

// ---- per-view measurement ----------------------------------------------------------------
/** A single (recent-input-filtered) layout-shift: its score and the time it occurred (ms). */
export interface LayoutShift { value: number; time: number; }

let clsShifts: LayoutShift[] = [], inpWorst = 0, viewStart = 0;
let observers: PerformanceObserver[] = [];
let routeChangeMo: MutationObserver | null = null;
// Bumped on every view start; the route_change settle-loop captures its generation and bails the
// moment a newer view supersedes it — so overlapping navigations can't leak observers or let a
// stale loop report/cross-attribute a value.
let measureGen = 0;

// CLS per web-vitals' "session window" rule (pure, unit-testable — no PerformanceObserver here):
// walk the (already recent-input-filtered) layout-shift entries in time order; a shift joins the
// current window if it is < 1000 ms after the previous shift AND < 5000 ms after the window's first
// shift, otherwise it opens a NEW window. CLS is the MAX window sum — not the naive cumulative total,
// which over-counts long-lived views.
export function sessionWindowCls(entries: LayoutShift[]): number {
  const sorted = entries.slice().sort((a, b) => a.time - b.time);
  let max = 0, cur = 0, first = 0, prev = 0, open = false;
  for (const e of sorted) {
    if (open && e.time - prev < 1000 && e.time - first < 5000) {
      cur += e.value;
    } else {
      cur = e.value; first = e.time; open = true;
    }
    prev = e.time;
    if (cur > max) max = cur;
  }
  return max;
}

function safeObserve(type: string, cb: (entries: PerformanceEntryList) => void, buffered = false): void {
  try {
    if (typeof PerformanceObserver === "undefined") return;
    const supported = (PerformanceObserver as any).supportedEntryTypes as string[] | undefined;
    if (supported && !supported.includes(type)) return;
    const po = new PerformanceObserver((list) => { try { cb(list.getEntries()); } catch { /* ignore */ } });
    po.observe({ type, buffered } as any);
    observers.push(po);
  } catch { /* ignore */ }
}

function startViewMeasurement(started: ViewDescriptor): void {
  // reset accumulators for the new view and invalidate any in-flight prior-view settle loop
  clsShifts = []; inpWorst = 0; viewStart = perfNow();
  const gen = ++measureGen;
  for (const o of observers) { try { o.disconnect(); } catch { /* ignore */ } }
  observers = [];
  if (routeChangeMo) { try { routeChangeMo.disconnect(); } catch { /* ignore */ } routeChangeMo = null; }

  // Only the LANDING (hard) view replays buffered entries: pre-init layout-shifts (the page's
  // load-time shifts, usually the dominant CLS) and interactions are counted even though they fired
  // before initPhoton ran. A SOFT view must NOT replay — a buffered observer would wrongly fold prior
  // views' shifts into this one — so it sees only entries after its own navStart.
  const buffered = started.nav === "hard";

  // CLS: record layout-shifts (excluding recent-input) as {value,time}; finalizeViewVitals folds them
  // with the session-window rule (sessionWindowCls) rather than a naive cumulative sum.
  safeObserve("layout-shift", (entries) => {
    for (const e of entries as any[]) if (!e.hadRecentInput) clsShifts.push({ value: e.value || 0, time: e.startTime || 0 });
  }, buffered);
  // INP: worst event-timing interaction latency since this view started.
  safeObserve("event", (entries) => {
    for (const e of entries as any[]) {
      const dur = e.duration || 0;
      if (dur > inpWorst) inpWorst = dur;
    }
  }, buffered);
  // route_change: nav → largest content paint before the view goes quiet.
  scheduleRouteChange(gen);
}

// DOM-settle heuristic: watch for content growth; declare "settled" after a quiet window with no
// long tasks / mutations, then report route_change = last-significant-activity - viewStart. The
// `gen` guard makes the loop a no-op once a newer view has started (rapid soft-nav), so it can
// never leak its observer or report a value cross-attributed to a later view.
function scheduleRouteChange(gen: number): void {
  try {
    if (typeof MutationObserver === "undefined" || typeof requestAnimationFrame === "undefined") return;
    const startedAt = viewStart;               // this view's start — not the shared global, which the next view resets
    let lastActivity = perfNow();
    const QUIET_MS = 500, MAX_MS = 10_000;
    const mo = new MutationObserver(() => { lastActivity = perfNow(); });
    routeChangeMo = mo;
    mo.observe(document.documentElement, { childList: true, subtree: true });
    safeObserve("longtask", () => { lastActivity = perfNow(); });
    safeObserve("resource", () => { lastActivity = perfNow(); });

    const check = () => {
      if (gen !== measureGen) { try { mo.disconnect(); } catch {} return; }  // superseded by a newer view
      const now = perfNow();
      if (now - lastActivity >= QUIET_MS || now - startedAt >= MAX_MS) {
        try { mo.disconnect(); } catch {}
        if (routeChangeMo === mo) routeChangeMo = null;
        const value = Math.max(0, Math.round(lastActivity - startedAt));
        try { hooks?.reportVital({ n: "route_change", v: value }); } catch {}
        return;
      }
      requestAnimationFrame(check);
    };
    requestAnimationFrame(check);
  } catch { /* ignore */ }
}

// Finalize the outgoing view's continuous vitals. Called by S5 wiring on each route change and on
// pagehide (for the last view), before flushing.
export function finalizeViewVitals(): void {
  try {
    const cls = sessionWindowCls(clsShifts);
    if (cls > 0) hooks?.reportVital({ n: "CLS", v: cls });
    if (inpWorst > 0) hooks?.reportVital({ n: "INP", v: Math.round(inpWorst) });
    // Idempotent: visibilitychange(hidden) AND pagehide both fire flushCurrent on tab close, so a
    // second call must not re-report the same accumulated CLS/INP (which would double-count them).
    clsShifts = []; inpWorst = 0;
  } catch { /* ignore */ }
}

function patchHistory(): void {
  try {
    if (typeof history === "undefined") return;
    const wrap = (name: "pushState" | "replaceState") => {
      const orig = history[name];
      if (typeof orig !== "function") return;
      history[name] = function (this: History, ...args: any[]) {
        const r = (orig as any).apply(this, args);
        try { maybeRotate(); } catch { /* never throw */ }
        return r;
      } as any;
    };
    wrap("pushState");
    wrap("replaceState");
    addEventListener("popstate", () => { try { maybeRotate(); } catch {} });
  } catch { /* never throw */ }
}

export function initSpa(h: SpaHooks): void {
  try {
    hooks = h;
    lastKey = keyOf(pathOf());
    startViewMeasurement(currentView());   // begin measuring the landing view's CLS/INP (its route_change is dropped by index.ts — soft navs only)
    patchHistory();
  } catch { /* never throw */ }
}

export function trackView(_route?: string): void { try { maybeRotate(); } catch { /* never throw */ } }
