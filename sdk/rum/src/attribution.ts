// Opt-in module: only pulled in when `opts.attribution === true` (dynamic `import()` from
// index.ts), so the core bundle stays tree-shaken and under the 5 KB budget. Uses the
// `web-vitals/attribution` builds, which populate `metric.attribution` for LCP/INP/CLS with
// debugging detail beyond the plain value.
import { onLCP, onINP, onCLS } from "web-vitals/attribution";

export interface AttrVital {
  n: string;
  v: number;
  attr?: Record<string, string | number | undefined>;
}

export interface AttrBeacon {
  vital: (v: AttrVital) => void;
}

// Round finite numbers only; drop NaN/undefined so JSON.stringify omits the key (defensive on
// the read side too — photon-core skips missing sub-parts).
function num(v: number | undefined): number | undefined {
  return typeof v === "number" && Number.isFinite(v) ? Math.round(v) : undefined;
}

export function initAttribution(b: AttrBeacon): void {
  onLCP((m) => {
    const a = m.attribution;
    b.vital({
      n: "LCP",
      v: Math.round(m.value),
      attr: {
        element: a.element,
        url: a.url,
        ttfb: num(a.timeToFirstByte),
        rld: num(a.resourceLoadDelay),
        rlt: num(a.resourceLoadDuration),
        erd: num(a.elementRenderDelay),
      },
    });
  });
  onINP((m) => {
    const a = m.attribution;
    b.vital({
      n: "INP",
      v: Math.round(m.value),
      attr: {
        target: a.interactionTarget,
        id: num(a.inputDelay),
        pd: num(a.processingDuration),
        pr: num(a.presentationDelay),
      },
    });
  });
  onCLS((m) => {
    const a = m.attribution;
    b.vital({ n: "CLS", v: m.value, attr: { source: a.largestShiftTarget } });
  });
}
