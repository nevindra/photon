import { describe, it, expect } from "vitest";
import { sessionId } from "../src/session";

describe("sessionId", () => {
  it("is stable across repeated calls within the idle window", () => {
    const a = sessionId();
    const b = sessionId();
    expect(a).toBe(b);
    expect(typeof a).toBe("string");
    expect(a.length).toBeGreaterThan(0);
  });
});
// viewId moved to view.ts and now rotates per navigation — see view.test.ts.
