import { describe, it, expect, vi } from "vitest";
import { newTraceId, newSpanId, traceparent, pageTraceId, matchesTarget } from "../src/trace";

describe("trace ids", () => {
  it("mints 32-hex trace ids and 16-hex span ids (lowercase)", () => {
    expect(newTraceId()).toMatch(/^[0-9a-f]{32}$/);
    expect(newSpanId()).toMatch(/^[0-9a-f]{16}$/);
  });
  it("builds a W3C traceparent (version 00, sampled 01)", () => {
    expect(traceparent("a".repeat(32), "b".repeat(16))).toBe(`00-${"a".repeat(32)}-${"b".repeat(16)}-01`);
  });
  it("returns a stable pageview trace id across calls", () => {
    expect(pageTraceId()).toBe(pageTraceId());
    expect(pageTraceId()).toMatch(/^[0-9a-f]{32}$/);
  });
});

describe("matchesTarget", () => {
  it("defaults to same-origin only", () => {
    // jsdom origin is http://localhost:3000 by default
    expect(matchesTarget("/api/x")).toBe(true);
    expect(matchesTarget(`${location.origin}/api/x`)).toBe(true);
    expect(matchesTarget("https://other.example.com/x")).toBe(false);
  });
  it("matches exact-origin strings and regexes", () => {
    expect(matchesTarget("https://api.example.com/v1", ["https://api.example.com"])).toBe(true);
    expect(matchesTarget("https://api.example.com/v1", ["https://nope.example.com"])).toBe(false);
    expect(matchesTarget("https://x.example.com/api/v1", [/\/api\//])).toBe(true);
  });
  it("rejects unparseable urls without throwing", () => {
    // Unterminated IPv6 host literal: genuinely fails WHATWG URL parsing (unlike "::::", which
    // resolves fine as a relative path against location.href).
    expect(matchesTarget("http://[::1", ["same-origin"])).toBe(false);
  });
});

describe("per-view trace id", () => {
  it("mints a distinct trace id per view, cached on the view descriptor, on rotate", async () => {
    vi.resetModules();
    const view = await import("../src/view");
    view.initView("/", "/");
    const { pageTraceId, bindTraceToViews } = await import("../src/trace");
    bindTraceToViews();                 // subscribe: each rotate mints a fresh id on the new descriptor
    const first = pageTraceId();
    expect(first).toMatch(/^[0-9a-f]{32}$/);
    expect(view.currentView().traceId).toBe(first);
    const { started } = view.rotateView("/next", "/next");
    const second = view.currentView().traceId;
    expect(second).toMatch(/^[0-9a-f]{32}$/);
    expect(second).not.toBe(first);
    expect(started.traceId).toBe(second);   // the rotated descriptor carries the fresh id
  });
});
