export function makeBeacon(endpoint: string, base: () => object) {
  const url = endpoint.replace(/\/$/, "") + "/api/rum";
  let vitals: any[] = [], errors: any[] = [];
  const flush = () => {
    if (!vitals.length && !errors.length) return;
    try {
      const body = JSON.stringify({ ...base(), vitals, errors });
      vitals = []; errors = [];
      const blob = new Blob([body], { type: "text/plain" });
      if (!navigator.sendBeacon?.(url, blob)) fetch(url, { method: "POST", body, keepalive: true, headers: { "content-type": "text/plain" } }).catch(() => {});
    } catch { /* swallow */ }
  };
  addEventListener("visibilitychange", () => { if (document.visibilityState === "hidden") flush(); });
  addEventListener("pagehide", flush);
  return { vital: (v: any) => vitals.push(v), error: (e: any) => errors.push(e), flush };
}
