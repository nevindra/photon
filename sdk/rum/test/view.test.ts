import { describe, it, expect, vi, beforeEach } from "vitest";
import { initView, rotateView, currentView, onViewChange, endedDuration } from "../src/view";

describe("view lifecycle", () => {
  beforeEach(() => { initView("/", "/"); });

  it("initView is the landing view: seq 0, nav hard, no prevRoute", () => {
    const v = initView("/home", "/home");
    expect(v.seq).toBe(0);
    expect(v.nav).toBe("hard");
    expect(v.prevRoute).toBeUndefined();
    expect(currentView().id).toBe(v.id);
  });

  it("rotateView mints a new id, increments seq, carries prevRoute, marks soft", () => {
    const first = currentView();
    const { ended, started } = rotateView("/next", "/next");
    expect(ended.id).toBe(first.id);
    expect(started.id).not.toBe(first.id);
    expect(started.seq).toBe(first.seq + 1);
    expect(started.prevRoute).toBe(first.route);
    expect(started.nav).toBe("soft");
    expect(currentView().id).toBe(started.id);
  });

  it("notifies onViewChange listeners with (ended, started)", () => {
    const cb = vi.fn();
    onViewChange(cb);
    const { ended, started } = rotateView("/x", "/x");
    expect(cb).toHaveBeenCalledWith(ended, started);
  });

  it("endedDuration measures ms from navStart", () => {
    const v = currentView();
    expect(endedDuration(v, v.navStart + 500)).toBe(500);
  });
});
