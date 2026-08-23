<script setup lang="ts">
// The Backend Issues filter panel: a FIXED set of six facet sections, fetched in one call by
// `useBackendIssueFacets`. Direct sibling of `rum/RumErrorFilters.vue` — same single-state model
// (checked state derives from the `query` prop; this component owns no selection state), same
// thin adapter over `FacetSection`/`FacetValueRow`, same `toggle`/`only`/`clear` emits for the
// view to translate through `queryLang`'s writers.
//
// The six fields mirror `ISSUE_FACET_FIELDS` (`crates/photon-query/src/span_issues.rs`) in order.
// They are backend questions, not frontend ones: what threw, where it threw, which environment
// and host, and what the caller saw.
import { computed } from 'vue'
import { FacetSection, FacetValueRow } from '@/components/ui/facet'
import { useBackendIssueFacets } from '@/lib/services/servicesQueries'
import { facetChecked, fieldConstraintCount } from '@/lib/core/queryLang'

const props = defineProps<{
  service: string
  query: string
  startNs: string
  endNs: string
}>()

const emit = defineEmits<{
  toggle: [{ field: string; value: string }]
  only: [{ field: string; value: string }]
  clear: [string]
}>()

const FIELDS = [
  'error.type',
  'name',
  'kind_text',
  'deployment.environment',
  'host.name',
  'http.status_code',
] as const

// Facet field names are the grammar's own field names, which is what makes a click here
// translatable into a term. `name` is the grammar's operation field — labelled as such, since
// a section headed "name" says nothing.
const LABELS: Record<string, string> = {
  'error.type': 'error.type',
  name: 'operation',
  kind_text: 'span kind',
  'deployment.environment': 'environment',
  'host.name': 'host',
  'http.status_code': 'http.status_code',
}

const facetsQuery = useBackendIssueFacets(
  () => props.service,
  () => props.query,
  () => props.startNs,
  () => props.endNs,
)
const facets = computed(() => facetsQuery.data.value?.facets ?? {})

// Per-field track-fill maxima — each section's fills are self-relative to its own busiest value.
function maxCount(field: string): number {
  let m = 0
  for (const v of facets.value[field]?.values ?? []) if (v.count > m) m = v.count
  return m
}
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
      :label="LABELS[field] ?? field"
      :active="fieldConstraintCount(query, field) > 0"
      :count="fieldConstraintCount(query, field)"
      :loading="facetsQuery.isLoading.value"
      :fetching="facetsQuery.isFetching.value && !facetsQuery.isLoading.value"
      :empty="!facets[field]?.values.length"
      empty-text="No values"
      :clear-data-test="`bif-clear-${field}`"
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
        :data-test="`bif-value-${field}-${v.value}`"
        @toggle="emit('toggle', { field, value: v.value })"
        @only="emit('only', { field, value: v.value })"
      />
    </FacetSection>
  </aside>
</template>
