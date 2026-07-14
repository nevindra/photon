<script setup lang="ts">
// The RUM errors list's whole filter panel: a FIXED set of six facet sections (unlike
// LogsFilters' open-ended Fields catalog) — exception.type / error.kind / browser.route /
// browser.name / device.type / network.connection, the same six dimensions
// `GET /api/rum/errors/facets` returns in one response (see `crates/photon-api/src/rum.rs`'s
// `ERROR_FACET_FIELDS`). One `useRumErrorFacets` call fetches all six at once — there's no
// per-field open/close state or dynamic catalog to manage, so this stays a thin adapter over
// `FacetSection`/`FacetValueRow`, mirroring LogsFilters' single-state model: checked state
// derives purely from the `query` prop via `facetChecked`, this component owns no selection
// state of its own, and clicks just emit intent (`toggle` / `only` / `clear`) for
// RumErrorsView to translate into query edits via the shared `queryLang` writers.
//
// Deviation from the task brief: the brief's `:count`/`:active` used the facet's total VALUE
// COUNT (`facets[field]?.values.length`), which would show a badge even with nothing filtered.
// Every other adapter (LogsFilters, FacetCatalog/FacetFieldGroup) uses
// `fieldConstraintCount(query, field)` — "how many terms constrain this field" — for both the
// "N" badge and Clear-affordance visibility, so that's what this mirrors instead.
import { computed } from 'vue'
import { FacetSection, FacetValueRow } from '@/components/ui/facet'
import { useRumErrorFacets } from '@/lib/rum/rumQueries'
import { facetChecked, fieldConstraintCount } from '@/lib/core/queryLang'

const props = defineProps<{
  app: string
  query: string
  startNs: string
  endNs: string
}>()

const emit = defineEmits<{
  toggle: [{ field: string; value: string }]
  only: [{ field: string; value: string }]
  clear: [string]
}>()

// Mirrors the backend's `ERROR_FACET_FIELDS` order exactly (rum.rs).
const FIELDS = [
  'exception.type',
  'error.kind',
  'browser.route',
  'browser.name',
  'device.type',
  'network.connection',
] as const

const facetsQuery = useRumErrorFacets(
  () => props.app,
  () => props.query,
  () => props.startNs,
  () => props.endNs,
)
const facets = computed(() => facetsQuery.data.value?.facets ?? {})

// Per-field track-fill maxima — each section's fills are self-relative to its own busiest value
// (mirrors LogsFilters/FacetFieldGroup's per-field `maxCount`).
function maxCount(field: string): number {
  let m = 0
  for (const v of facets.value[field]?.values ?? []) if (v.count > m) m = v.count
  return m
}
// `FacetValueRow`'s `share` prop type-infers as `number | undefined` (a Vue runtime prop
// declaration `{ type: Number, default: null }` in a plain-JS SFC doesn't widen to include
// `null` for vue-tsc's consumer-side type) — return `undefined`, not `null`, for the no-fill case.
function shareFor(field: string, count: number): number | undefined {
  const max = maxCount(field)
  return max > 0 ? count / max : undefined
}
</script>

<template>
  <aside class="w-[210px] shrink-0 space-y-1 overflow-y-auto min-h-0">
    <FacetSection
      v-for="field in FIELDS"
      :key="field"
      :label="field"
      :active="fieldConstraintCount(query, field) > 0"
      :count="fieldConstraintCount(query, field)"
      :loading="facetsQuery.isLoading.value"
      :fetching="facetsQuery.isFetching.value && !facetsQuery.isLoading.value"
      :empty="!(facets[field]?.values.length)"
      empty-text="No values"
      :clear-data-test="`rf-clear-${field}`"
      @clear="emit('clear', field)"
    >
      <FacetValueRow
        v-for="v in facets[field]?.values ?? []"
        :key="v.value"
        mono
        :label="v.value"
        :count="v.count"
        :checked="facetChecked(query, field, v.value)"
        :share="shareFor(field, v.count)"
        :data-test="`rf-value-${field}-${v.value}`"
        @toggle="emit('toggle', { field, value: v.value })"
        @only="emit('only', { field, value: v.value })"
      />
    </FacetSection>
  </aside>
</template>
