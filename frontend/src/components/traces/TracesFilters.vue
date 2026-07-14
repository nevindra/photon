<script setup>
// TracesFilters — the traces adapter of the unified filter panel. Composes the shared
// `ui/facet` primitives (FacetSection · FacetValueRow · FacetCatalog) into the traces left
// column, replacing TracesQuickFilters + SpanFacetRail as one component. Unlike logs'
// prop-driven adapter it SELF-FETCHES its pinned Service/Status/Kind sections (one
// `useTracesFacet` per section) so their counts always reflect the live window/query rather than
// only the currently-loaded result page. All page-specific wiring (which columns each section
// facets, the grammar aliases, the enum domains, the hidden-field set) lives here; the shared
// primitives own presentation + the catalog mechanics.
import { computed } from 'vue'
import { FacetSection, FacetValueRow, FacetCatalog } from '@/components/ui/facet'
import { useTracesFacet, useTracesFields } from '@/lib/traces/tracesQueries'
import {
  facetChecked,
  fieldConstraintCount,
  fieldValues,
  negatedFieldValues,
  removeFieldAll,
} from '@/lib/core/queryLang'
import { api } from '@/lib/core/api'

const props = defineProps({
  query: { type: String, default: '' },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
})
// Single-state model (see docs/superpowers/.../facet-single-state-model.md): each value has
// exactly ONE state, checked or unchecked — there is no separate "exclude" toggle. A row
// click/Enter/Space toggles that state (`toggle-value`); unchecking a value writes a `-field:v`
// exclusion under the hood (see `toggleFacetValue`), it isn't a distinct user action. `clear-field`
// resets a whole field to its default (every value checked). `only-value` (hover action) narrows
// the field to exactly one value.
const emit = defineEmits(['toggle-value', 'clear-field', 'only-value'])

const NS = 1_000_000n
const toNs = (ms) => (BigInt(Math.round(ms)) * NS).toString()
const startNs = computed(() => toNs(props.startMs))
const endNs = computed(() => toNs(props.endMs))

// Each section facets against the query with its OWN grammar field stripped of BOTH signs
// (`removeFieldAll`, not `removeField`) — the "a facet doesn't filter itself out of its own
// breakdown" pattern the catalog uses too. Stripping only positive terms would leave this field's
// own `-field:x` exclusions in the facet query, which would silently skew (or drop values out of)
// its own breakdown; every value must stay listed with an honest count regardless of which of its
// own values are currently checked/unchecked. Note this strips the grammar ALIAS
// (`service`/`status`/`kind`), not the facet COLUMN (`service.name`/`status_text`/`kind_text`) the
// query below is actually run against.
const serviceQuery = computed(() => removeFieldAll(props.query, 'service'))
const statusQuery = computed(() => removeFieldAll(props.query, 'status'))
const kindQuery = computed(() => removeFieldAll(props.query, 'kind'))

// --- Service: a genuine open-ended facet (service names aren't a fixed enum), fetched on the real
// `service.name` column so the backend returns actual services + counts. Checkbox toggle/clear use
// the grammar ALIAS `service` (never `service.name`) to match the search bar's own convention —
// the two names resolve to the same column server-side, so they're interchangeable in query text.
const serviceFacet = useTracesFacet('service.name', serviceQuery, startNs, endNs)
const serviceLoading = computed(() => !!serviceFacet.isLoading.value)
const serviceFetching = computed(() => !!serviceFacet.isFetching?.value)
// Explicitly-constrained values — included (`service:x`) OR excluded (`-service:x`) — must stay
// pinned/visible even if faceting no longer surfaces them (an included service outside the fetched
// top-N, or an excluded one that dropped out) — synthesized with a null count so the row renders
// without a (misleading) count and no meter.
const serviceValues = computed(() => {
  const fetched = serviceFacet.data.value?.values ?? []
  const known = new Set(fetched.map((v) => v.value))
  const included = fieldValues(props.query, 'service')
  const excluded = negatedFieldValues(props.query, 'service')
  const pinned = new Set([...included, ...excluded].filter((v) => !known.has(v)))
  const pins = [...pinned].map((v) => ({ value: v, count: null }))
  return [...fetched, ...pins]
})

// --- Status/Kind: CLOSED enums the grammar special-cases (`status:`/`kind:` resolve to a numeric
// status/kind code match, not a column match). The resolver REJECTS faceting on `status`/`kind`
// themselves, so counts come from the underlying human-readable columns (`status_text`/`kind_text`,
// stored upper-case, e.g. "ERROR"/"SERVER"). Rather than surface that raw casing, these two
// sections always render the FULL fixed keyword domain (no "unknown status" to discover, so no
// empty state) and look up each keyword's count case-insensitively; toggling emits the lower-case
// grammar keyword (`status:error`, not `status_text:ERROR`) so the resulting query reads exactly
// the way the search bar's own autocomplete would write it.
const STATUS_KEYS = ['ok', 'error', 'unset']
const STATUS_LABELS = { ok: 'Ok', error: 'Error', unset: 'Unset' }
const KIND_KEYS = ['server', 'client', 'internal', 'producer', 'consumer']
const KIND_LABELS = {
  server: 'Server',
  client: 'Client',
  internal: 'Internal',
  producer: 'Producer',
  consumer: 'Consumer',
}

function countsByKeyword(values) {
  const out = {}
  for (const v of values ?? []) out[String(v.value).toLowerCase()] = v.count
  return out
}

const statusFacet = useTracesFacet('status_text', statusQuery, startNs, endNs)
const statusLoading = computed(() => !!statusFacet.isLoading.value)
const statusFetching = computed(() => !!statusFacet.isFetching?.value)
const statusCounts = computed(() => countsByKeyword(statusFacet.data.value?.values))

const kindFacet = useTracesFacet('kind_text', kindQuery, startNs, endNs)
const kindLoading = computed(() => !!kindFacet.isLoading.value)
const kindFetching = computed(() => !!kindFacet.isFetching?.value)
const kindCounts = computed(() => countsByKeyword(kindFacet.data.value?.values))

// Baseline-meter share is self-relative to each section's own max count (matching the catalog's
// per-field bars); a null count (a pinned out-of-top-N value) or an empty section yields no meter.
const maxServiceCount = computed(() => {
  let m = 0
  for (const v of serviceValues.value) if (typeof v.count === 'number' && v.count > m) m = v.count
  return m
})
const maxStatusCount = computed(() => Math.max(0, ...STATUS_KEYS.map((k) => statusCounts.value[k] ?? 0)))
const maxKindCount = computed(() => Math.max(0, ...KIND_KEYS.map((k) => kindCounts.value[k] ?? 0)))
function shareOf(count, max) {
  if (count == null || max <= 0) return null
  return count / max
}

// --- Fields catalog: the field catalog for the window (refetches as startMs/endMs change), driving
// the shared FacetCatalog with the traces facet fetcher + the spans-specific hidden-field set.
const fieldsQuery = useTracesFields(startNs, endNs)
const catalogFields = computed(() => fieldsQuery.data.value ?? [])

// Fields already surfaced elsewhere (the pinned quick-filters / the waterfall itself) or that
// aren't meaningfully groupable (ids, raw timing, the wide attributes/events/links blobs).
// `status_text` and `kind_text` are deliberately NOT hidden — those are the human-readable enums
// worth faceting on — and long-tail/promoted attributes are never hidden.
const HIDDEN = new Set([
  'service.name',
  'status_code',
  'kind',
  'start_time_nanos',
  'end_time_nanos',
  'duration_nanos',
  'attributes',
  'events',
  'links',
  'trace_id',
  'span_id',
  'parent_span_id',
  'scope_name',
])

// Single-state checkbox reads (`facetChecked`) and the "N constraints" count driving Clear's
// visibility + the section badge (`fieldConstraintCount` — positive includes + negated excludes).
function isChecked(field, value) {
  return facetChecked(props.query, field, value)
}
function activeCount(field) {
  return fieldConstraintCount(props.query, field)
}
</script>

<template>
  <div class="flex flex-col">
    <FacetSection
      label="Service"
      :loading="serviceLoading"
      :fetching="serviceFetching"
      :empty="serviceValues.length === 0"
      empty-text="No services"
      :active="activeCount('service') > 0"
      :count="activeCount('service')"
      clear-data-test="qf-clear-service"
      loading-data-test="qf-service-skeleton"
      @clear="emit('clear-field', 'service')"
    >
      <FacetValueRow
        v-for="v in serviceValues"
        :key="v.value"
        mono
        :label="v.value"
        :count="v.count"
        :checked="isChecked('service', v.value)"
        :share="shareOf(v.count, maxServiceCount)"
        :data-test="`qf-service-${v.value}`"
        @toggle="emit('toggle-value', { field: 'service', value: v.value })"
        @only="emit('only-value', { field: 'service', value: v.value })"
      />
    </FacetSection>

    <FacetSection
      label="Status"
      :loading="statusLoading"
      :fetching="statusFetching"
      :active="activeCount('status') > 0"
      :count="activeCount('status')"
      clear-data-test="qf-clear-status"
      loading-data-test="qf-status-skeleton"
      @clear="emit('clear-field', 'status')"
    >
      <FacetValueRow
        v-for="key in STATUS_KEYS"
        :key="key"
        :label="STATUS_LABELS[key]"
        :count="statusCounts[key] ?? 0"
        :checked="isChecked('status', key)"
        :share="shareOf(statusCounts[key] ?? 0, maxStatusCount)"
        :label-class="key === 'error' && isChecked('status', key) ? 'text-sev-error' : ''"
        :data-test="`qf-status-${key}`"
        @toggle="emit('toggle-value', { field: 'status', value: key })"
        @only="emit('only-value', { field: 'status', value: key })"
      />
    </FacetSection>

    <FacetSection
      label="Kind"
      :loading="kindLoading"
      :fetching="kindFetching"
      :active="activeCount('kind') > 0"
      :count="activeCount('kind')"
      clear-data-test="qf-clear-kind"
      loading-data-test="qf-kind-skeleton"
      @clear="emit('clear-field', 'kind')"
    >
      <FacetValueRow
        v-for="key in KIND_KEYS"
        :key="key"
        :label="KIND_LABELS[key]"
        :count="kindCounts[key] ?? 0"
        :checked="isChecked('kind', key)"
        :share="shareOf(kindCounts[key] ?? 0, maxKindCount)"
        :data-test="`qf-kind-${key}`"
        @toggle="emit('toggle-value', { field: 'kind', value: key })"
        @only="emit('only-value', { field: 'kind', value: key })"
      />
    </FacetSection>

    <FacetCatalog
      :fields="catalogFields"
      :facet-fn="api.tracesFacet"
      query-key-prefix="traces-facet"
      :hidden="HIDDEN"
      :query="query"
      :start-ns="startNs"
      :end-ns="endNs"
      @toggle="emit('toggle-value', $event)"
      @only="emit('only-value', $event)"
      @clear="emit('clear-field', $event)"
    />
  </div>
</template>
