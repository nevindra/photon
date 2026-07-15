import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { initTracing } from "../src/tracing";
import { currentView } from "../src/view";

describe("initTracing", () => {
  let origFetch: typeof globalThis.fetch;
  beforeEach(() => {
    origFetch = globalThis.fetch;
  });
  afterEach(() => {
    globalThis.fetch = origFetch;
    vi.restoreAllMocks();
  });

  it("mints the current view's trace id on init", () => {
    initTracing();
    expect(currentView().traceId).toMatch(/^[0-9a-f]{32}$/);
  });

  it("injects traceparent on same-origin fetch, not cross-origin", async () => {
    const seen: Array<[string, Headers]> = [];
    globalThis.fetch = vi.fn(async (input: any, init?: any) => {
      const h = new Headers(init?.headers);
      seen.push([String(input), h]);
      return new Response("");
    }) as any;
    initTracing();
    await fetch("/api/thing");
    await fetch("https://other.example.com/api/thing");
    expect(seen[0][1].get("traceparent")).toMatch(/^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/);
    expect(seen[1][1].get("traceparent")).toBeNull();
  });

  it("injects traceparent on same-origin XHR", () => {
    initTracing();
    const xhr = new XMLHttpRequest();
    const spy = vi.spyOn(xhr, "setRequestHeader");
    xhr.open("GET", "/api/thing");
    xhr.send();
    expect(spy).toHaveBeenCalledWith("traceparent", expect.stringMatching(/^00-/));
  });

  it("never throws even if fetch is hostile", async () => {
    globalThis.fetch = vi.fn(() => { throw new Error("boom"); }) as any;
    initTracing();
    expect(() => fetch("/api/x").catch(() => {})).not.toThrow();
  });
});

import { initPhoton } from "../src/index";

// jsdom's Blob implementation doesn't have .text()/.arrayBuffer() (only slice/size/type), so read
// it back via FileReader instead (same workaround as test/beacon.test.ts).
function readBlobText(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsText(blob);
  });
}

describe("beacon carries trace when tracing is on", () => {
  it("includes a 32-hex trace on the flushed beacon body", async () => {
    // Capture the actual read promise per sendBeacon call rather than racing a fixed tick count —
    // FileReader's callback timing varies with event-loop load (e.g. XHR/network activity from
    // earlier tests in this file), so a blind setTimeout(r, 0) is flaky; awaiting the real
    // promise(s) is deterministic while keeping the same assertion.
    const pending: Promise<string>[] = [];
    (navigator as any).sendBeacon = (_url: string, blob: Blob) => {
      pending.push(readBlobText(blob));
      return true;
    };
    initPhoton({ app: "web", endpoint: "https://photon.example.com", key: "pk", tracing: true });
    await import("../src/tracing"); // ensure the dynamic chunk resolved and initTracing ran
    await Promise.resolve();
    window.dispatchEvent(new ErrorEvent("error", { message: "boom", error: new Error("boom") }));
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("pagehide"));
    const bodies = await Promise.all(pending);
    const merged = bodies.join("");
    expect(merged).toMatch(/"trace":"[0-9a-f]{32}"/);
  });
});
