<script setup>
// The logs explorer's whole filter panel: the pinned Services + Severity quick-filters and the
// Fields catalog, composed from the shared `ui/facet/` primitives (FacetSection, FacetValueRow,
// FacetCatalog). Replaces the old prop-driven FilterRail + self-fetching FacetRail with one
// adapter — the single-state model is unchanged: checked state derives purely from the `query`
// prop via `facetChecked`, and this component owns no selection state of its own, it just emits
// intent (`toggle-value` / `only-value` / `clear-field`) for LogsView to translate into query
// edits. See .superpowers/sdd/facet-single-state-model.md and the unified-filter-panel design.
import { computed } from 'vue'
import { FacetSection, FacetValueRow, FacetCatalog } from '@/components/ui/facet'
import { SEVERITIES, severity, severityClasses } from '@/lib/core/format'
import { facetChecked, fieldConstraintCount } from '@/lib/core/queryLang'
import { useFields } from '@/lib/logs/logsQueries'
import { api } from '@/lib/core/api'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  query: { type: String, default: '' },
  services: { type: Array, default: () => [] },
  serviceCounts: { type: Object, default: () => ({}) },
  severityCounts: { type: Object, default: () => ({}) },
  startMs: { type: Number, required: true },
  endMs: { type: Number, required: true },
})

const emit = defineEmits(['toggle-value', 'only-value', 'clear-field'])

// Literal ring-color classes per severity tone — kept as a static lookup (not built dynamically)
// so Tailwind's content scanner can see every class string. Copied verbatim from FilterRail.
const RING_CLASS = {
  neutral: 'ring-border',
  warn: 'ring-sev-warn',
  error: 'ring-sev-error',
  fatal: 'ring-sev-fatal',
}

// Fields already surfaced as pinned quick-filters above (Services = service.name, Severity =
// severity_text/number), plus columns that aren't meaningfully groupable (timestamps, the
// free-text body, the raw attributes map). Hidden so the catalog stays a list of things worth
// faceting on and never duplicates the pinned sections. Copied verbatim from FacetRail.
const HIDDEN = new Set([
  'service.name',
  'severity_text',
  'severity_number',
  'timestamp',
  'observed_timestamp',
  'body',
  'attributes',
])

// Nanosecond window bounds (as strings) derived from the ms props — threaded into the field
// catalog fetch + the FacetCatalog facet requests/cache keys. Copied from FacetRail's toNs.
const NS = 1_000_000n
const toNs = (ms) => (BigInt(Math.round(ms)) * NS).toString()
const startNs = computed(() => toNs(props.startMs))
const endNs = computed(() => toNs(props.endMs))

// Field catalog for the current window (manifest-only, no data scan). Passed down to FacetCatalog,
// which owns the grouping/open-state/facet fan-out.
const { data: fieldsData } = useFields(startNs, endNs)

// Per-section track-fill maxima: each section's fills are self-relative to its own busiest
// value. Guard the empty case so `share` degrades to null (no fill) rather than dividing by zero.
const maxServiceCount = computed(() =>
  Math.max(0, ...props.services.map((svc) => props.serviceCounts[svc] ?? 0)),
)
const maxSeverityCount = computed(() =>
  Math.max(0, ...SEVERITIES.map((s) => props.severityCounts[s.key] ?? 0)),
)
function serviceShare(svc) {
  return maxServiceCount.value > 0 ? (props.serviceCounts[svc] ?? 0) / maxServiceCount.value : null
}
function severityShare(key) {
  return maxSeverityCount.value > 0 ? (props.severityCounts[key] ?? 0) / maxSeverityCount.value : null
}
</script>

<template>
  <div class="flex flex-col">
    <!-- Services (pinned) — prop-driven counts from LogsView's per-section facet fetch. -->
    <FacetSection
      label="Services"
      :active="fieldConstraintCount(query, 'service') > 0"
      :count="fieldConstraintCount(query, 'service')"
      clear-data-test="fr-clear-service"
      @clear="emit('clear-field', 'service')"
    >
      <FacetValueRow
        v-for="svc in services"
        :key="svc"
        mono
        :label="svc"
        :count="serviceCounts[svc] ?? 0"
        :checked="facetChecked(query, 'service', svc)"
        :share="serviceShare(svc)"
        :data-test="`fr-service-${svc}`"
        @toggle="emit('toggle-value', { field: 'service', value: svc })"
        @only="emit('only-value', { field: 'service', value: svc })"
      />
    </FacetSection>

    <!-- Severity (pinned) — a coloured dot marker (via #marker) instead of the default Checkbox. -->
    <FacetSection
      label="Severity"
      :active="fieldConstraintCount(query, 'level') > 0"
      :count="fieldConstraintCount(query, 'level')"
      clear-data-test="fr-clear-level"
      @clear="emit('clear-field', 'level')"
    >
      <FacetValueRow
        v-for="s in SEVERITIES"
        :key="s.key"
        :label="severity(s.key).label"
        :count="severityCounts[s.key] ?? 0"
        :checked="facetChecked(query, 'level', s.key)"
        :share="severityShare(s.key)"
        neutral-fill
        :data-test="`fr-severity-${s.key}`"
        @toggle="emit('toggle-value', { field: 'level', value: s.key })"
        @only="emit('only-value', { field: 'level', value: s.key })"
      >
        <template #marker="{ checked }">
          <span
            :class="
              cn(
                'h-2.5 w-2.5 shrink-0 rounded-full',
                checked ? severityClasses(s.key).solid : cn('bg-transparent ring-1 ring-inset', RING_CLASS[s.tone]),
              )
            "
          />
        </template>
      </FacetValueRow>
    </FacetSection>

    <!-- Fields catalog — the shared primitive owns search/grouping/open-state/facet fan-out. -->
    <FacetCatalog
      :fields="fieldsData ?? []"
      :facet-fn="api.facet"
      query-key-prefix="facet"
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
