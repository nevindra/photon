import { describe, it, expect, vi } from "vitest";
import { collectErrors } from "../src/errors";

function fireError(message: string, filename = "app.js", lineno = 1) {
  const err = new Error(message);
  const event = new ErrorEvent("error", { message, filename, lineno, error: err });
  window.dispatchEvent(event);
}

function fireRejection(reason: unknown) {
  const event = new Event("unhandledrejection") as any;
  event.reason = reason;
  window.dispatchEvent(event);
}

describe("collectErrors", () => {
  it("dedups + rate-limits identical errors: 25 dispatches yield <=20 push calls", () => {
    const push = vi.fn();
    collectErrors(push);

    for (let i = 0; i < 25; i++) {
      fireError("boom");
    }

    expect(push.mock.calls.length).toBeLessThanOrEqual(20);
    expect(push.mock.calls.length).toBeLessThan(25); // dedup actually kicked in
    expect(push).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "exception", type: "Error", msg: "boom", src: "app.js" })
    );
  });

  it("does not rate-limit distinct errors: different messages each push", () => {
    const push = vi.fn();
    collectErrors(push);

    fireError("first unique error");
    fireError("second unique error");
    fireError("third unique error");

    expect(push).toHaveBeenCalledTimes(3);
  });

  it("never propagates a throw raised inside the push callback (error path)", () => {
    const push = vi.fn(() => {
      throw new Error("push blew up");
    });
    collectErrors(push);

    expect(() => fireError("push-throws-" + Math.random())).not.toThrow();
    expect(push).toHaveBeenCalledTimes(1);
  });

  it("never propagates a throw raised inside the push callback (unhandledrejection path)", () => {
    const push = vi.fn(() => {
      throw new Error("push blew up");
    });
    collectErrors(push);

    expect(() => fireRejection(new Error("rejected-" + Math.random()))).not.toThrow();
    expect(push).toHaveBeenCalledTimes(1);
  });

  it("also collects unhandledrejection events", () => {
    const push = vi.fn();
    collectErrors(push);

    fireRejection(new Error("some rejection " + Math.random()));

    expect(push).toHaveBeenCalledTimes(1);
    expect(push.mock.calls[0][0]).toMatchObject({ kind: "unhandledrejection", type: "Error" });
  });
});
