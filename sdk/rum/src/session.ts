const rid = () => (crypto.randomUUID?.() ?? String(Math.random()).slice(2) + Date.now().toString(36));
const IDLE = 30 * 60_000, MAX = 4 * 3600_000;
let sid = rid(), started = perfNow(), last = started;
export const viewId = rid();
function perfNow() { return typeof performance !== "undefined" ? performance.timeOrigin + performance.now() : Date.now(); }
export function sessionId(): string {
  const now = perfNow();
  if (now - last > IDLE || now - started > MAX) { sid = rid(); started = now; }
  last = now;
  return sid;
}
