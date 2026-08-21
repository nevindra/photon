// Discovery for the global environment dimension: which `deployment.environment` values actually
// appear in the active window. `deployment.environment` is a promoted attribute (see
// photon-core/src/default.toml), so this is an ordinary facet — there is no environment registry
// to read, because Photon never invents an environment, it only reports the ones producers declare.
//
// Faceted over BOTH logs (`GET /api/facet`) and spans (`GET /api/traces/facet`) and unioned: the
// dimension is global, and an install that ships traces but not logs (APM-first, the common case)
// would otherwise never see the picker. `allSettled` — one signal being empty, or rejecting, must
// not take the other's values down with it. There is no metrics equivalent to add: the metrics
// label endpoint is per-metric, so there is no whole-signal facet to union in.
//
// Deliberately NOT routed through `logsQueries.useFacet`: that helper appends the full context
// fragment, environment included, so picking `prod` would collapse this facet to a single value
// and hide the very picker that was just used. The Site (`tenant`) term IS applied — environments
// are per-Site, and central's lens should list the ones that Site reports.
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, keepPreviousData } from '@tanstack/vue-query'
import { api } from '@/lib/core/api'
import type { FacetResult } from '@/lib/core/api'
import { tenantQueryTerm } from '@/lib/core/context'

export const ENVIRONMENT_FIELD = 'deployment.environment'

// A cap well above any real install's environment count — this facet feeds a dropdown, not a
// table, and a runaway-cardinality attribute (someone stamping a build id here) should be bounded
// rather than rendered.
const MAX_ENVIRONMENTS = 20

// Union the two per-signal facets, summing counts so the most-used environment sorts first.
function unionValues(results: PromiseSettledResult<FacetResult>[]): string[] {
  const counts = new Map<string, number>()
  for (const r of results) {
    if (r.status !== 'fulfilled') continue
    for (const v of r.value?.values ?? []) {
      counts.set(v.value, (counts.get(v.value) ?? 0) + (v.count ?? 0))
    }
  }
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, MAX_ENVIRONMENTS)
    .map(([name]) => name)
}

export function useEnvironments(
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    queryKey: computed(() => [
      'environments',
      String(toValue(startNs)),
      String(toValue(endNs)),
      tenantQueryTerm(),
    ]),
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<string[]> => {
      const q = tenantQueryTerm() ?? ''
      const [s, e] = [toValue(startNs), toValue(endNs)]
      return unionValues(
        await Promise.allSettled([
          api.facet(ENVIRONMENT_FIELD, q, s, e, MAX_ENVIRONMENTS, { signal }),
          api.tracesFacet(ENVIRONMENT_FIELD, q, s, e, MAX_ENVIRONMENTS, { signal }),
        ]),
      )
    },
    // The set of environments changes on the timescale of a deploy pipeline, not a page view, and
    // this fires on every route — hold it fresh for a minute and keep the last list on screen so
    // the picker never flickers away mid-window-change.
    staleTime: 60_000,
    gcTime: 5 * 60_000,
    placeholderData: keepPreviousData,
  })
}
