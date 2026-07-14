<script setup>
// The whole Fields catalog: a field-search box, the promoted/attributes hybrid grouping, the
// per-field open-state (multiple fields open at once), the auto-open-of-constrained-fields
// watch, and the dynamic `useQueries` fan-out that fetches values for exactly the open fields.
// Data-source-agnostic: the page adapter injects `fields` (already fetched), a `facetFn`
// (`api.facet` or `api.tracesFacet`), the `queryKeyPrefix` for the cache, and the per-page
// `hidden` set — everything page-specific stays in the adapter.
import { ref, reactive, computed, watch } from 'vue'
import { useQueries, keepPreviousData } from '@tanstack/vue-query'
import { ChevronRight, Search } from 'lucide-vue-next'
import FacetFieldGroup from './FacetFieldGroup.vue'
import { fieldConstraintCount, removeFieldAll } from '@/lib/core/queryLang'

const props = defineProps({
  // The field catalog `[{ name, kind }]`, already fetched by the adapter (kinds: fixed |
  // promoted | attribute).
  fields: { type: Array, default: () => [] },
  // Injected facet fetcher, called `(name, strippedQuery, startNs, endNs, 50, { signal })`
  // → Promise<{ values, capped }>. This is `api.facet` (logs) or `api.tracesFacet` (traces).
  facetFn: { type: Function, required: true },
  // Cache-key prefix so results interop with any other caller of the same endpoint
  // (`'facet'` for logs, `'traces-facet'` for traces).
  queryKeyPrefix: { type: String, default: 'facet' },
  // Fields already surfaced as pinned quick-filters (or not worth faceting) — never listed here.
  hidden: { type: Set, default: () => new Set() },
  // The current query string, and the (nanosecond, string) window bounds threaded into the
  // facet requests + cache keys.
  query: { type: String, default: '' },
  startNs: { type: String, default: '' },
  endNs: { type: String, default: '' },
})

// Forwarded straight up, unchanged, from the field groups — the adapter re-emits these as the
// page's `toggle-value` / `only-value` / `clear-field`.
const emit = defineEmits(['toggle', 'only', 'clear'])

const fieldFilter = ref('')
const open = reactive({}) // field name -> true when expanded (multiple may be open)
const attrsOpen = ref(false) // the Attributes long-tail group — collapsed by default

const searching = computed(() => fieldFilter.value.trim().length > 0)
const isPromoted = (f) => f.kind === 'fixed' || f.kind === 'promoted'

// Non-hidden fields matching the field-search. When searching, everything renders as one flat
// list (the Kibana escape hatch — no field is ever unreachable behind a collapsed group).
const filtered = computed(() => {
  const q = fieldFilter.value.trim().toLowerCase()
  return props.fields.filter((f) => !props.hidden.has(f.name) && (!q || f.name.toLowerCase().includes(q)))
})
const promotedFields = computed(() => filtered.value.filter(isPromoted))
const attributeFields = computed(() => filtered.value.filter((f) => !isPromoted(f)))

// Auto-open fields that already carry a constraint — an include OR an exclude (e.g. seeded from
// the URL) — so a returning user immediately sees the filters in effect. Runs whenever the field
// catalog (re)loads (a time-window change), mirroring the old `loadFields` scan. A constrained
// ATTRIBUTE field also forces the Attributes group open so the seeded filter is actually visible.
watch(
  () => props.fields,
  (list) => {
    for (const f of list) {
      if (props.hidden.has(f.name)) continue
      if (fieldConstraintCount(props.query, f.name) > 0 && !open[f.name]) {
        open[f.name] = true
        if (!isPromoted(f)) attrsOpen.value = true
      }
    }
  },
  { immediate: true },
)

const openFieldNames = computed(() => Object.keys(open))

// One facet request per OPEN field. `useFacet`/`useTracesFacet` wrap a single `useQuery` and
// can't be called a runtime-varying number of times from one setup(), so this uses `useQueries`
// with the same key/queryFn shape those composables use — results interop with any other caller.
// We facet against the query with THIS field's own terms stripped, BOTH signs (`removeFieldAll`),
// so a field's own includes/excludes never skew its own breakdown. Options are unified to the
// traces flavour (short staleTime avoids open/close thrash, gcTime keeps a just-closed field
// warm, `keepPreviousData` dims-while-refetching instead of skeletoning) for both pages.
const facetResults = useQueries({
  queries: computed(() =>
    openFieldNames.value.map((name) => {
      const q = removeFieldAll(props.query, name)
      return {
        queryKey: [props.queryKeyPrefix, name, q, props.startNs, props.endNs, 50],
        queryFn: ({ signal }) => props.facetFn(name, q, props.startNs, props.endNs, 50, { signal }),
        staleTime: 30_000,
        gcTime: 5 * 60_000,
        placeholderData: keepPreviousData,
      }
    }),
  ),
})

// field name -> { loading, values, capped, fetching }, index-matched to openFieldNames (both are
// built from the same list in the same tick). `loading` (isPending) is true only before ANY data
// has loaded; `fetching` drives the dim-while-refetching treatment on a populated list.
const entries = computed(() => {
  const map = {}
  openFieldNames.value.forEach((name, i) => {
    const r = facetResults.value[i]
    map[name] = {
      loading: !!r?.isPending,
      values: r?.data?.values ?? [],
      capped: r?.data?.capped ?? false,
      fetching: !!r?.isFetching,
    }
  })
  return map
})

function toggleField(name) {
  if (open[name]) delete open[name]
  else open[name] = true
}
</script>

<template>
  <section class="flex flex-col border-t border-border">
    <div class="px-4 pb-2 pt-4">
      <div class="mb-2 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Fields</div>
      <div class="relative">
        <Search class="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-muted-foreground" />
        <input
          v-model="fieldFilter"
          type="text"
          placeholder="Filter fields…"
          aria-label="Filter fields"
          class="w-full rounded-sm border border-border bg-transparent py-1 pl-7 pr-2 font-mono text-xs text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
        />
      </div>
    </div>

    <div class="flex flex-col gap-0.5 px-2 pb-4">
      <p v-if="filtered.length === 0" class="px-2 py-1 text-[11px] text-muted-foreground">
        {{ fieldFilter.trim() ? 'No fields match' : 'No fields' }}
      </p>

      <!-- Searching: flatten both groups into one filtered list, no group headers. -->
      <template v-if="searching">
        <FacetFieldGroup
          v-for="f in filtered"
          :key="f.name"
          :field="f"
          :open="!!open[f.name]"
          :values="entries[f.name]?.values ?? []"
          :loading="entries[f.name]?.loading ?? false"
          :fetching="entries[f.name]?.fetching ?? false"
          :capped="entries[f.name]?.capped ?? false"
          :active-count="fieldConstraintCount(query, f.name)"
          :query="query"
          @toggle-open="toggleField"
          @toggle="emit('toggle', $event)"
          @only="emit('only', $event)"
          @clear="emit('clear', $event)"
        />
      </template>

      <!-- Calm default: promoted fields first, the raw-attribute long tail folded away. -->
      <template v-else>
        <template v-if="promotedFields.length">
          <div
            data-test="facet-group-promoted"
            class="px-1.5 pb-0.5 pt-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/80"
          >
            Promoted
          </div>
          <FacetFieldGroup
            v-for="f in promotedFields"
            :key="f.name"
            :field="f"
            :open="!!open[f.name]"
            :values="entries[f.name]?.values ?? []"
            :loading="entries[f.name]?.loading ?? false"
            :fetching="entries[f.name]?.fetching ?? false"
            :capped="entries[f.name]?.capped ?? false"
            :active-count="fieldConstraintCount(query, f.name)"
            :query="query"
            @toggle-open="toggleField"
            @toggle="emit('toggle', $event)"
            @only="emit('only', $event)"
            @clear="emit('clear', $event)"
          />
        </template>

        <template v-if="attributeFields.length">
          <button
            type="button"
            data-test="facet-group-attributes"
            :aria-expanded="attrsOpen"
            class="mt-0.5 flex items-center gap-1.5 rounded-sm px-1.5 py-1.5 text-left text-muted-foreground transition-colors motion-reduce:transition-none hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            @click="attrsOpen = !attrsOpen"
          >
            <ChevronRight
              :class="[
                'h-3 w-3 shrink-0 transition-transform motion-reduce:transition-none',
                attrsOpen && 'rotate-90',
              ]"
            />
            <span class="flex-1 text-[10px] font-medium uppercase tracking-wider">Attributes</span>
            <span
              class="shrink-0 rounded-full bg-muted px-1.5 text-[10px] font-medium leading-4 tabular-nums text-muted-foreground"
            >
              {{ attributeFields.length }}
            </span>
          </button>

          <template v-if="attrsOpen">
            <FacetFieldGroup
              v-for="f in attributeFields"
              :key="f.name"
              :field="f"
              :open="!!open[f.name]"
              :values="entries[f.name]?.values ?? []"
              :loading="entries[f.name]?.loading ?? false"
              :fetching="entries[f.name]?.fetching ?? false"
              :capped="entries[f.name]?.capped ?? false"
              :active-count="fieldConstraintCount(query, f.name)"
              :query="query"
              @toggle-open="toggleField"
              @toggle="emit('toggle', $event)"
              @only="emit('only', $event)"
              @clear="emit('clear', $event)"
            />
          </template>
        </template>
      </template>
    </div>
  </section>
</template>
