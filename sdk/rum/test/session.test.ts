import { describe, it, expect } from "vitest";
import { sessionId, viewId } from "../src/session";

describe("sessionId", () => {
  it("is stable across repeated calls within the idle window", () => {
    const a = sessionId();
    const b = sessionId();
    const c = sessionId();

    expect(a).toBe(b);
    expect(b).toBe(c);
    expect(typeof a).toBe("string");
    expect(a.length).toBeGreaterThan(0);
  });
});

describe("viewId", () => {
  it("is a stable non-empty string for the module lifetime", () => {
    expect(typeof viewId).toBe("string");
    expect(viewId.length).toBeGreaterThan(0);
    expect(viewId).toBe(viewId);
  });
});
