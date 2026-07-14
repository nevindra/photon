// Tiny shared holder so index.ts's base() can read the pageview trace id WITHOUT statically
// importing the (opt-in, lazily-loaded) tracing/id-gen code — keeping the core bundle tree-shaken.
export const traceState: { id?: string } = {};
