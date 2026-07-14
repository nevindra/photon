<script setup>
// One expandable field in the catalog: a header (chevron · monospace name · active-count badge
// or hovered `kind`) and, when open, an optional per-field value-search, the value list rendered
// with FacetValueRow, the top-50 cap note, and a per-field Clear. The parent (FacetCatalog) owns
// the open-state and the per-field facet fetch and hands the fetched values in as props; this
// component owns only its local value-search string. It re-emits value intent up with the field
// name attached so the catalog can forward it unchanged.
import { ref, computed } from 'vue'
import { Skeleton } from '@/components/ui/skeleton'
import { ChevronRight, X } from 'lucide-vue-next'
import FacetValueRow from './FacetValueRow.vue'
import { facetChecked, fieldValues, negatedFieldValues } from '@/lib/core/queryLang'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  // { name, kind } from the /fields catalog. `kind` is `fixed | promoted | attribute`.
  field: { type: Object, required: true },
  // Whether this field is expanded (owned by the parent).
  open: { type: Boolean, default: false },
  // The fetched top values `[{ value, count }]` for this field (owned by the parent).
  values: { type: Array, default: () => [] },
  // Before ANY values have loaded → skeleton; refetching over loaded values → dim.
  loading: { type: Boolean, default: false },
  fetching: { type: Boolean, default: false },
  // The backend truncated to the top 50 → show the cap note and always offer the value-search.
  capped: { type: Boolean, default: false },
  // Number of constraints on this field (includes + excludes) → drives the badge + Clear.
  activeCount: { type: Number, default: 0 },
  // The current query string. Checked-state (`facetChecked`) and the pinned/floated constrained
  // values are derived from it — the cleaner of the two documented APIs, since the display-value
  // logic already needs the query's include/exclude sets, so a `checkedFor` fn would still need
  // the query passed alongside it.
  query: { type: String, default: '' },
})

const emit = defineEmits(['toggle-open', 'toggle', 'only', 'clear'])

const VALUE_FILTER_THRESHOLD = 8 // offer a per-field value search once a field has more than this
const search = ref('')

const name = computed(() => props.field.name)

// Meter share is computed against THIS field's own max fetched count, so each field's bars are
// self-relative (a value dominant within its field reads full-width regardless of other fields).
const maxCount = computed(() => {
  let m = 0
  for (const v of props.values) if (typeof v.count === 'number' && v.count > m) m = v.count
  return m
})
function shareFor(v) {
  if (v.count == null || maxCount.value <= 0) return null
  return v.count / maxCount.value
}

function isChecked(value) {
  return facetChecked(props.query, name.value, value)
}

// The rows to render: the fetched top values, plus any explicitly-constrained value — included
// OR excluded — that fell outside the top-50 (synthesized with a null count so it stays visible
// and a user-touched value never vanishes), filtered by the per-field search, with constrained
// values floated to the top. This is the exact `displayValues` logic ported from FacetRail.vue.
const displayValues = computed(() => {
  const fetched = props.values ?? []
  const included = fieldValues(props.query, name.value)
  const excluded = negatedFieldValues(props.query, name.value)
  const known = new Set(fetched.map((v) => v.value))
  const pinned = new Set([...included, ...excluded].filter((v) => !known.has(v)))
  const extras = [...pinned].map((v) => ({ value: v, count: null }))
  let rows = [...extras, ...fetched]
  const q = search.value.trim().toLowerCase()
  if (q) rows = rows.filter((r) => r.value.toLowerCase().includes(q))
  // Stable sort keeps count-desc order within each group; constrained values float to the top.
  return rows
    .map((r, i) => [r, i])
    .sort((a, b) => {
      const sa = included.includes(a[0].value) || excluded.includes(a[0].value) ? 0 : 1
      const sb = included.includes(b[0].value) || excluded.includes(b[0].value) ? 0 : 1
      return sa - sb || a[1] - b[1]
    })
    .map(([r]) => r)
})

const showValueFilter = computed(
  () => !props.loading && (props.values.length > VALUE_FILTER_THRESHOLD || props.capped),
)
</script>

<template>
  <div>
    <button
      type="button"
      :data-test="`facet-field-${name}`"
      :aria-expanded="open"
      :class="[
        'group flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left transition-colors motion-reduce:transition-none hover:bg-muted/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
        activeCount > 0 && 'bg-muted/40',
      ]"
      @click="emit('toggle-open', name)"
    >
      <ChevronRight
        :class="[
          'h-3 w-3 shrink-0 text-muted-foreground transition-transform motion-reduce:transition-none',
          open && 'rotate-90',
        ]"
      />
      <span class="flex-1 truncate font-mono text-xs text-foreground">{{ name }}</span>
      <span
        v-if="activeCount > 0"
        class="shrink-0 rounded-full bg-foreground px-1.5 text-[10px] font-medium leading-4 tabular-nums text-background"
      >
        {{ activeCount }}
      </span>
      <span
        v-else
        class="shrink-0 text-[10px] text-muted-foreground opacity-0 transition-opacity motion-reduce:transition-none group-hover:opacity-100"
      >
        {{ field.kind }}
      </span>
    </button>

    <!-- Body: tree-guided by a left border, indented under the chevron. -->
    <div v-if="open" class="mb-1 ml-3.5 border-l border-border pl-2">
      <div v-if="showValueFilter" class="relative mb-1 pr-1">
        <input
          v-model="search"
          type="text"
          placeholder="Filter values…"
          :aria-label="`Filter ${name} values`"
          class="w-full rounded-sm border border-border bg-transparent px-2 py-0.5 font-mono text-[11px] text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
          @click.stop
        />
      </div>

      <div v-if="loading" class="flex flex-col gap-1 px-1.5 py-1" aria-hidden="true">
        <Skeleton v-for="i in 3" :key="i" class="h-3.5" :style="{ width: `${70 - i * 12}%` }" />
      </div>

      <template v-else>
        <ul
          :class="
            cn(
              'flex flex-col gap-0.5 transition-opacity duration-[var(--motion-base)] motion-reduce:transition-none',
              fetching && 'opacity-60',
            )
          "
        >
          <li v-for="v in displayValues" :key="v.value">
            <FacetValueRow
              mono
              :label="v.value"
              :count="v.count"
              :checked="isChecked(v.value)"
              :share="shareFor(v)"
              :data-test="`facet-value-${v.value}`"
              @toggle="emit('toggle', { field: name, value: v.value })"
              @only="emit('only', { field: name, value: v.value })"
            />
          </li>

          <li v-if="displayValues.length === 0" class="px-1.5 py-1 text-[11px] text-muted-foreground">
            {{ search.trim() ? 'No values match' : 'No values' }}
          </li>
          <li v-if="capped" class="px-1.5 pt-0.5 text-[10px] text-muted-foreground">Top 50 by count</li>
        </ul>

        <button
          v-if="activeCount > 0"
          type="button"
          :data-test="`facet-field-${name}-clear`"
          class="mt-0.5 flex items-center gap-1 rounded-sm px-1.5 py-0.5 text-[10px] text-muted-foreground transition-colors motion-reduce:transition-none hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          @click.stop="emit('clear', name)"
        >
          <X class="h-2.5 w-2.5" />
          Clear
        </button>
      </template>
    </div>
  </div>
</template>
