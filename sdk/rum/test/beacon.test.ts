import { describe, it, expect, beforeEach, vi } from "vitest";
import { makeBeacon } from "../src/beacon";

// jsdom's Blob implementation doesn't have .text()/.arrayBuffer() (only slice/size/type),
// so read it back via FileReader instead.
function readBlobText(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });
}

describe("makeBeacon", () => {
  let sendBeacon: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    sendBeacon = vi.fn(() => true);
    // jsdom does not implement navigator.sendBeacon
    (navigator as any).sendBeacon = sendBeacon;
  });

  it("flushes queued vitals/errors on pagehide and posts the frozen beacon shape", async () => {
    const base = () => ({
      app: "web-storefront",
      key: "pk_live_abc",
      session: "018f-session",
      view: { id: "018f-view", route: "/checkout", path: "/checkout" },
      ctx: { ua: "Mozilla/5.0 (iPhone) Safari", conn: "4g" },
    });
    const beacon = makeBeacon("https://photon.example.com", base);

    beacon.vital({ n: "LCP", v: 4300 });
    beacon.vital({ n: "CLS", v: 0.06 });
    beacon.error({
      kind: "exception",
      type: "TypeError",
      msg: "x is undefined",
      stack: "...",
      src: "checkout.js",
      line: 214,
    });

    window.dispatchEvent(new Event("pagehide"));

    expect(sendBeacon).toHaveBeenCalledTimes(1);
    const [url, blob] = sendBeacon.mock.calls[0];
    expect(url).toBe("https://photon.example.com/api/rum");
    expect(blob).toBeInstanceOf(Blob);

    const text = await readBlobText(blob);
    const parsed = JSON.parse(text);

    // Frozen shape: app, key, session, view, vitals, errors (+ whatever base() contributes, e.g. ctx).
    expect(Object.keys(parsed).sort()).toEqual(
      ["app", "key", "session", "view", "ctx", "vitals", "errors"].sort()
    );
    expect(parsed.app).toBe("web-storefront");
    expect(parsed.key).toBe("pk_live_abc");
    expect(parsed.session).toBe("018f-session");
    expect(parsed.view).toEqual({ id: "018f-view", route: "/checkout", path: "/checkout" });
    expect(parsed.ctx).toEqual({ ua: "Mozilla/5.0 (iPhone) Safari", conn: "4g" });
    expect(parsed.vitals).toEqual([
      { n: "LCP", v: 4300 },
      { n: "CLS", v: 0.06 },
    ]);
    expect(parsed.errors).toEqual([
      {
        kind: "exception",
        type: "TypeError",
        msg: "x is undefined",
        stack: "...",
        src: "checkout.js",
        line: 214,
      },
    ]);
  });

  it("clears the queue after a flush so a second pagehide sends nothing new", async () => {
    const base = () => ({ app: "web", key: "pk_1", session: "s1", view: { id: "v1", route: "/", path: "/" } });
    const beacon = makeBeacon("https://photon.example.com", base);

    beacon.vital({ n: "LCP", v: 1000 });
    window.dispatchEvent(new Event("pagehide"));
    expect(sendBeacon).toHaveBeenCalledTimes(1);

    window.dispatchEvent(new Event("pagehide"));
    expect(sendBeacon).toHaveBeenCalledTimes(1); // no second call: queue was empty
  });

  it("sends nothing when there are no queued vitals or errors", () => {
    const base = () => ({ app: "web", key: "pk_1", session: "s1", view: { id: "v1", route: "/", path: "/" } });
    makeBeacon("https://photon.example.com", base);

    window.dispatchEvent(new Event("pagehide"));

    expect(sendBeacon).not.toHaveBeenCalled();
  });

  it("falls back to fetch keepalive when sendBeacon is unavailable or fails", () => {
    (navigator as any).sendBeacon = vi.fn(() => false);
    const fetchSpy = vi.fn(() => Promise.resolve(new Response(null, { status: 204 })));
    vi.stubGlobal("fetch", fetchSpy);

    const base = () => ({ app: "web", key: "pk_1", session: "s1", view: { id: "v1", route: "/", path: "/" } });
    const beacon = makeBeacon("https://photon.example.com", base);
    beacon.vital({ n: "TTFB", v: 120 });

    window.dispatchEvent(new Event("pagehide"));

    expect(fetchSpy).toHaveBeenCalledTimes(1);
    const [url, init] = fetchSpy.mock.calls[0];
    expect(url).toBe("https://photon.example.com/api/rum");
    expect(init).toMatchObject({ method: "POST", keepalive: true });

    vi.unstubAllGlobals();
  });

  it("never throws into the host app even when sendBeacon itself throws", () => {
    (navigator as any).sendBeacon = vi.fn(() => {
      throw new Error("sendBeacon exploded");
    });

    const base = () => ({ app: "web", key: "pk_1", session: "s1", view: { id: "v1", route: "/", path: "/" } });
    const beacon = makeBeacon("https://photon.example.com", base);
    beacon.vital({ n: "LCP", v: 1000 });

    expect(() => window.dispatchEvent(new Event("pagehide"))).not.toThrow();
  });
});
