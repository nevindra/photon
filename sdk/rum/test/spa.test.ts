import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { sessionWindowCls } from "../src/spa";

// jsdom provides window/history/location. Reset module state per test via dynamic import.
async function freshSpa() {
  vi.resetModules();
  const view = await import("../src/view");
  view.initView(location.pathname, location.pathname);
  const spa = await import("../src/spa");
  return { view, spa };
}

describe("spa soft-navigation detector", () => {
  beforeEach(() => { history.replaceState(null, "", "/"); });
  afterEach(() => { vi.restoreAllMocks(); delete (globalThis as any).requestAnimationFrame; });

  it("rotates the view on a pushState that changes the path", async () => {
    const { spa } = await freshSpa();
    const onRouteChange = vi.fn();
    spa.initSpa({ onRouteChange, reportVital: () => {} });
    history.pushState(null, "", "/dashboard");
    expect(onRouteChange).toHaveBeenCalledTimes(1);
    const [, started] = onRouteChange.mock.calls[0];
    expect(started.path).toBe("/dashboard");
    expect(started.nav).toBe("soft");
  });

  it("does NOT rotate on a query-only change", async () => {
    const { spa } = await freshSpa();
    const onRouteChange = vi.fn();
    spa.initSpa({ onRouteChange, reportVital: () => {} });
    history.pushState(null, "", "/?tab=2");
    history.replaceState(null, "", "/?tab=3");
    expect(onRouteChange).not.toHaveBeenCalled();
  });

  it("rotates on popstate (back/forward) when the path changed", async () => {
    const { spa } = await freshSpa();
    const onRouteChange = vi.fn();
    // Capture the pre-patch replaceState so we can move the URL the way a browser back/forward does
    // (the browser updates the URL BEFORE firing popstate) WITHOUT triggering the patched rotation.
    const rawReplace = history.replaceState.bind(history);
    spa.initSpa({ onRouteChange, reportVital: () => {} });
    history.pushState(null, "", "/a");   // rotate #1
    history.pushState(null, "", "/b");   // rotate #2
    onRouteChange.mockClear();
    rawReplace(null, "", "/a");          // browser back: URL is now /a, no patched rotation
    dispatchEvent(new PopStateEvent("popstate"));
    expect(onRouteChange).toHaveBeenCalledTimes(1);
    expect(onRouteChange.mock.calls[0][1].path).toBe("/a");
  });

  it("does NOT rotate on a popstate that lands on the same route key", async () => {
    const { spa } = await freshSpa();
    const onRouteChange = vi.fn();
    const rawReplace = history.replaceState.bind(history);
    spa.initSpa({ onRouteChange, reportVital: () => {} });
    history.pushState(null, "", "/a");   // rotate
    onRouteChange.mockClear();
    rawReplace(null, "", "/a?x=1");      // browser back to a query-only state — same path "/a"
    dispatchEvent(new PopStateEvent("popstate"));
    expect(onRouteChange).not.toHaveBeenCalled();
  });

  it("uses routeOf for the route-change key when provided", async () => {
    const { spa } = await freshSpa();
    const onRouteChange = vi.fn();
    spa.initSpa({ onRouteChange, reportVital: () => {}, routeOf: (p) => p.replace(/\/\d+/, "/:id") });
    history.pushState(null, "", "/p/1");
    history.pushState(null, "", "/p/2");   // same routeOf -> NO rotate
    expect(onRouteChange).toHaveBeenCalledTimes(1);
    expect(onRouteChange.mock.calls[0][1].route).toBe("/p/:id");
  });

  it("never throws if history is missing", async () => {
    const { spa } = await freshSpa();
    expect(() => spa.initSpa({ onRouteChange: () => { throw new Error("boom"); }, reportVital: () => {} })).not.toThrow();
    expect(() => history.pushState(null, "", "/z")).not.toThrow();
  });

  it("trackView rotates using the current location", async () => {
    const { spa } = await freshSpa();
    const onRouteChange = vi.fn();
    spa.initSpa({ onRouteChange, reportVital: () => {} });
    history.replaceState(null, "", "/manual");
    spa.trackView();
    expect(onRouteChange).toHaveBeenCalledTimes(1);
    expect(onRouteChange.mock.calls[0][1].path).toBe("/manual");
  });

  it("disconnects the prior view's route_change observer on the next navigation (no leak)", async () => {
    vi.resetModules();
    // jsdom has no rAF; provide a stub so scheduleRouteChange runs and creates a MutationObserver,
    // but never actually ticks the settle loop (we only assert the teardown, not a reported value).
    (globalThis as any).requestAnimationFrame = () => 1;
    const disconnectSpy = vi.spyOn(MutationObserver.prototype, "disconnect");
    const view = await import("../src/view");
    view.initView("/", "/");
    const spa = await import("../src/spa");
    spa.initSpa({ onRouteChange: () => {}, reportVital: () => {} }); // landing view's settle observer
    const before = disconnectSpy.mock.calls.length;
    history.pushState(null, "", "/leak-a"); // rotate → startViewMeasurement must tear down the prior observer
    expect(disconnectSpy.mock.calls.length).toBeGreaterThan(before);
    disconnectSpy.mockRestore();
    delete (globalThis as any).requestAnimationFrame;
  });

  it("reports route_change for a soft view and suppresses it for the hard/landing view", async () => {
    vi.resetModules();
    // Controllable clock + rAF: perfNow() deltas come only from performance.now() (timeOrigin is
    // constant), so mocking now() drives the settle loop; rAF callbacks queue and we flush by hand.
    let clock = 1000;
    vi.spyOn(performance, "now").mockImplementation(() => clock);
    let rafQueue: FrameRequestCallback[] = [];
    (globalThis as any).requestAnimationFrame = (cb: FrameRequestCallback) => { rafQueue.push(cb); return rafQueue.length; };
    const flushRaf = () => { for (let i = 0; i < 20 && rafQueue.length; i++) { const q = rafQueue; rafQueue = []; for (const cb of q) cb(0); } };

    const view = await import("../src/view");
    view.initView("/", "/");
    const spa = await import("../src/spa");

    // Mirror index.ts's gate: route_change is dropped while the current view is the hard landing view.
    const reported: string[] = [];
    const reportVital = (m: { n: string; v: number }) => {
      if (m.n === "route_change" && view.currentView().nav === "hard") return;
      reported.push(m.n);
    };
    spa.initSpa({ onRouteChange: () => {}, reportVital });

    // Settle the LANDING (hard) view's loop → route_change is computed but gated out (nav === "hard").
    clock += 800;
    flushRaf();
    expect(reported).not.toContain("route_change");

    // Soft-navigate → a new soft view; settle its loop → route_change now passes the gate.
    history.pushState(null, "", "/next");
    expect(view.currentView().nav).toBe("soft");
    clock += 800;
    flushRaf();
    expect(reported).toContain("route_change");
  });
});

describe("sessionWindowCls (web-vitals session-window rule)", () => {
  it("sums two shifts 800 ms apart into one window", () => {
    expect(sessionWindowCls([{ value: 0.1, time: 0 }, { value: 0.05, time: 800 }])).toBeCloseTo(0.15);
  });

  it("splits on a >1000 ms gap and returns the max window, not the total", () => {
    const cls = sessionWindowCls([
      { value: 0.10, time: 0 },
      { value: 0.05, time: 800 },   // < 1s after prev, < 5s after first → same window (sum 0.15)
      { value: 0.30, time: 2300 },  // 1500 ms after prev → NEW window (sum 0.30)
    ]);
    expect(cls).toBeCloseTo(0.30);   // max window wins
    expect(cls).not.toBeCloseTo(0.45); // NOT the naive cumulative total
  });

  it("caps a window at 5000 ms from its first entry", () => {
    // Shifts every 900 ms (each < 1s gap) but the run exceeds the 5s window cap, forcing a new window.
    const cls = sessionWindowCls([
      { value: 0.1, time: 0 },
      { value: 0.1, time: 900 },
      { value: 0.1, time: 1800 },
      { value: 0.1, time: 2700 },
      { value: 0.1, time: 3600 },
      { value: 0.1, time: 4500 },   // 4500 < 5000 → still window 1 (sum 0.6)
      { value: 0.1, time: 5400 },   // 5400 >= 5000 from first → NEW window (sum 0.1)
    ]);
    expect(cls).toBeCloseTo(0.6);
  });

  it("returns 0 for no shifts", () => {
    expect(sessionWindowCls([])).toBe(0);
  });
});
