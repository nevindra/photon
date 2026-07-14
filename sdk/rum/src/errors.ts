type Err = { kind: string; type: string; msg: string; stack: string; src: string; line?: number };
const seen = new Map<string, number>();
const MAX_PER_MIN = 20;
export function collectErrors(push: (e: Err) => void) {
  const handle = (kind: string, type: string, msg: string, stack: string, src = "", line?: number) => {
    try {
      const k = type + "|" + msg.replace(/\d+/g, "#") + "|" + src;
      const n = (seen.get(k) ?? 0) + 1;
      seen.set(k, n);
      if (n > MAX_PER_MIN) return; // client rate-limit an error loop
      push({ kind, type, msg, stack, src, line });
    } catch { /* never propagate */ }
  };
  addEventListener("error", (e) => handle("exception", (e.error?.name) ?? "Error", e.message ?? String(e.error), e.error?.stack ?? "", e.filename, e.lineno), true);
  addEventListener("unhandledrejection", (e: PromiseRejectionEvent) => {
    const r: any = e.reason;
    handle("unhandledrejection", r?.name ?? "UnhandledRejection", r?.message ?? String(r), r?.stack ?? "", "");
  });
  setInterval(() => seen.clear(), 60_000); // reset the rate-limit window
}
