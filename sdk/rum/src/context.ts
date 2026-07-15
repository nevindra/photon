export function context() {
  const c: any = (navigator as any).connection;
  return {
    ua: navigator.userAgent,
    conn: c?.effectiveType ?? "",
  };
}
