import { describe, it, expect, vi, beforeEach } from "vitest";
import { makeBeacon } from "../src/beacon";
import type { ViewDescriptor } from "../src/view";

function desc(over: Partial<ViewDescriptor> = {}): ViewDescriptor {
  return { id: "v1", route: "/a", path: "/a", navStart: 0, seq: 0, nav: "hard", ...over };
}

describe("per-view beacon", () => {
  let sent: any[];
  beforeEach(() => {
    sent = [];
    (globalThis as any).navigator = { sendBeacon: (_u: string, b: Blob) => { sent.push(b); return true; } };
    (globalThis as any).Blob = class { constructor(public parts: any[]) {} async text() { return this.parts.join(""); } };
    (globalThis as any).addEventListener = () => {};
  });

  it("stamps flushed items with the supplied (outgoing) descriptor, not the current one", async () => {
    const b = makeBeacon("https://x.test", () => ({ app: "app", key: "k", session: "s", ctx: {} }));
    b.vital({ n: "LCP", v: 1 });
    const viewA = desc({ id: "A", route: "/a" });
    b.flush(viewA);                              // flush for view A
    b.vital({ n: "INP", v: 2 });                 // now belongs to a later view
    b.flush(desc({ id: "B", route: "/b" }));
    const first = JSON.parse(await (sent[0] as any).text());
    expect(first.view.id).toBe("A");
    expect(first.view.route).toBe("/a");
    expect(first.vitals).toEqual([{ n: "LCP", v: 1 }]);
  });

  it("flush with an empty buffer sends nothing", () => {
    const b = makeBeacon("https://x.test", () => ({ app: "a", key: "k", session: "s", ctx: {} }));
    b.flush(desc());
    expect(sent.length).toBe(0);
  });

  it("sendImmediate emits a one-off beacon for a specific descriptor without touching the buffer", async () => {
    const b = makeBeacon("https://x.test", () => ({ app: "a", key: "k", session: "s", ctx: {} }));
    b.vital({ n: "CLS", v: 0.1 });               // stays buffered
    b.sendImmediate(desc({ id: "LATE" }), { vitals: [{ n: "LCP", v: 9 }] });
    const body = JSON.parse(await (sent[0] as any).text());
    expect(body.view.id).toBe("LATE");
    expect(body.vitals).toEqual([{ n: "LCP", v: 9 }]);
    // buffer still holds the CLS for a later flush:
    b.flush(desc({ id: "CUR" }));
    const second = JSON.parse(await (sent[1] as any).text());
    expect(second.vitals).toEqual([{ n: "CLS", v: 0.1 }]);
  });

  it("includes view.seq/prev/nav/dur and per-view trace when present", async () => {
    const b = makeBeacon("https://x.test", () => ({ app: "a", key: "k", session: "s", ctx: {} }));
    b.vital({ n: "route_change", v: 300 });
    b.flush(desc({ id: "V", route: "/r", path: "/r", seq: 2, prevRoute: "/p", nav: "soft", traceId: "abc", navStart: 0 }));
    const body = JSON.parse(await (sent[0] as any).text());
    expect(body.view).toMatchObject({ id: "V", route: "/r", path: "/r", seq: 2, prev: "/p", nav: "soft" });
    expect(typeof body.view.dur).toBe("number");
    expect(body.trace).toBe("abc");
  });
});
