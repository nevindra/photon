// The `:service` route param, decoded and trimmed, shared by every `/services/:service/*` view.
//
// `route.params` gives an array when a param repeats, and vue-router already percent-decodes —
// re-decoding a name containing a literal `%` would corrupt it. Both mistakes are easy to make once
// per view, which is why this is one function.
import { computed, type ComputedRef } from 'vue'
import { useRoute } from 'vue-router'

function one(v: string | string[] | undefined): string {
  return ((Array.isArray(v) ? v[0] : v) ?? '').trim()
}

export function useServiceParam(): ComputedRef<string> {
  const route = useRoute()
  return computed(() => one(route.params.service as string | string[] | undefined))
}

export function useRouteParam(name: string): ComputedRef<string> {
  const route = useRoute()
  return computed(() => one(route.params[name] as string | string[] | undefined))
}
