export function context() {
  const c: any = (navigator as any).connection;
  return {
    ua: navigator.userAgent,
    conn: c?.effectiveType ?? "",
  };
}
export function view(routeOf?: (p: string) => string) {
  const path = location.pathname;
  return { route: routeOf ? routeOf(path) : path, path };
}
