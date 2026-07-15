import { describe, it, expect, vi, beforeEach } from "vitest";

describe("initPhoton SPA wiring", () => {
  beforeEach(() => { vi.resetModules(); history.replaceState(null, "", "/"); });

  // Attribution-by-construction: an error buffered on the LANDING view must be flushed with the
  // landing descriptor when the user soft-navigates away — proving errors are attributed to the
  // view they happened on, not the one the user moved to. The flush is driven by the synchronous
  // `onViewChange` listener (fired inside rotateView, BEFORE spa.ts resets accumulators).
  it("attributes a buffered error to the view it happened on, not the next one", async () => {
    const posts: any[] = [];
    (globalThis as any).navigator = { sendBeacon: (_u: string, b: any) => { posts.push(b); return true; }, userAgent: "test" };
    (globalThis as any).Blob = class { constructor(public parts: any[]) {} async text() { return this.parts.join(""); } };
    const { initPhoton } = await import("../src/index");
    const { currentView } = await import("../src/view");
    initPhoton({ app: "a", endpoint: "https://x.test", key: "k" });
    const landing = currentView();
    const landingId = landing.id;
    const landingRoute = landing.route;
    // an error happens while the landing view is active...
    window.dispatchEvent(new ErrorEvent("error", { message: "attributed-boom", error: new Error("attributed-boom"), filename: "app.js", lineno: 42 }));
    // ...then the user soft-navigates away, which flushes the landing view's buffer.
    history.pushState(null, "", "/next");
    // the new view is a distinct pageview (id rotated)
    expect(currentView().id).not.toBe(landingId);
    // read back the beacon stamped with the LANDING descriptor (matched by unique view id, so it is
    // robust to any beacons from stale module instances left over by earlier tests in this file).
    const bodies = await Promise.all(posts.map((b) => b.text()));
    const landingBeacon = bodies.map((t) => JSON.parse(t)).find((p) => p.view.id === landingId);
    expect(landingBeacon).toBeTruthy();
    expect(landingBeacon.view.route).toBe(landingRoute);
    expect(landingBeacon.errors.some((e: any) => e.msg === "attributed-boom")).toBe(true);
  });

  // The flush + rotate mechanic itself: a soft navigation must flush the OUTGOING view's buffer as a
  // real beacon (stamped as the landing/hard view, seq 0) and start a genuinely new soft pageview
  // (fresh id, seq 1, prevRoute set). Complements the attribution test above, which checks *what*
  // rides the beacon; this checks that the rotation happens and the outgoing beacon actually goes out.
  it("flushes the outgoing view and rotates on soft navigation", async () => {
    const posts: any[] = [];
    (globalThis as any).navigator = { sendBeacon: (_u: string, b: any) => { posts.push(b); return true; }, userAgent: "test" };
    (globalThis as any).Blob = class { constructor(public parts: any[]) {} async text() { return this.parts.join(""); } };
    const { initPhoton } = await import("../src/index");
    const { currentView } = await import("../src/view");
    initPhoton({ app: "a", endpoint: "https://x.test", key: "k" });
    const landingId = currentView().id;
    // buffer something on the landing view so its flush emits a beacon...
    window.dispatchEvent(new ErrorEvent("error", { message: "outgoing-boom", error: new Error("outgoing-boom"), filename: "app.js", lineno: 1 }));
    // ...then soft-navigate: the OUTGOING (landing) view is flushed and a NEW soft view starts.
    history.pushState(null, "", "/next");
    // the rotation produced a genuinely new pageview
    const started = currentView();
    expect(started.id).not.toBe(landingId);
    expect(started.nav).toBe("soft");
    expect(started.seq).toBe(1);
    expect(started.prevRoute).toBe("/");
    // and the outgoing view's beacon actually went out, stamped as the landing (hard, seq 0) view
    const bodies = await Promise.all(posts.map((b) => b.text()));
    const outgoing = bodies.map((t) => JSON.parse(t)).find((p) => p.view.id === landingId);
    expect(outgoing).toBeTruthy();
    expect(outgoing.view.nav).toBe("hard");
    expect(outgoing.view.seq).toBe(0);
  });

  it("never throws when window APIs are missing", async () => {
    const { initPhoton } = await import("../src/index");
    expect(() => initPhoton({ app: "a", endpoint: "e", key: "k", sampleRate: 0 })).not.toThrow();
  });
});
