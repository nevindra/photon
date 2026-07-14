<script setup>
// The atom of the filter panel: one selectable value row, reused verbatim in the pinned
// sections (Services/Severity, Service/Status/Kind) AND in every expanded catalog field. The
// row is a single-state facet control (see lib/queryLang.js `facetChecked`) — `checked` = the
// value is IN the result set — but this component owns no query state: it just renders the
// state it's handed and emits intent (`toggle` / `only`) for a parent to translate into query
// edits. One place to style, one place to test, so the meter, counts, and Only affordance stay
// pixel-consistent from the pinned sections down through the catalog.
import { computed } from 'vue'
import { Checkbox } from '@/components/ui/checkbox'
import { Equal } from 'lucide-vue-next'
import { formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  // Row label (a service/attribute value, or a severity/status/kind human label).
  label: { type: String, default: '' },
  // Occurrence count shown at the right. `null` → render NO count AND no track fill (a
  // pinned out-of-top-N constrained value has an unknown count).
  count: { type: Number, default: null },
  // Single-state membership: checked = in the result set. Drives the marker + `aria-pressed`.
  checked: { type: Boolean, default: false },
  // Track-fill share, 0–1 (`count / maxCountInGroup`, computed by the caller) — the fraction of the
  // row width the soft distribution fill spans. `null`/absent → no fill. A null `count` also
  // suppresses the fill regardless of `share`.
  share: { type: Number, default: null },
  // Monospace the label (service names, attribute values) vs sans (severity/status/kind labels).
  mono: { type: Boolean, default: false },
  // Optional semantic accent for the label, e.g. `text-sev-error` for the Status "Error" row.
  // When set it overrides the default checked/unchecked tone entirely (both states).
  labelClass: { type: String, default: '' },
  // `data-test` for the row; the Only button gets `${dataTest}-only`.
  dataTest: { type: String, default: '' },
  // Use a neutral (uncoloured) track fill even when checked. For sections whose colour is already
  // meaningful — Logs Severity's per-level dot marker — this keeps the brand cyan out of the
  // severity zone (the "severity colour policy untouched" rule). Default: checked rows take a faint
  // brand tint that pairs with the cyan checkbox marker.
  neutralFill: { type: Boolean, default: false },
})

const emit = defineEmits(['toggle', 'only'])

// The fill is decorative: it renders only when both a real count and a share exist, and the count
// is ALWAYS shown alongside it (the Kibana guardrail — a dominant value that fills the whole row
// must never visually swallow the rest).
const showMeter = computed(() => props.count != null && props.share != null)
const meterWidth = computed(() => `${Math.max(0, Math.min(1, props.share ?? 0)) * 100}%`)
// Track-fill tone: a faint brand-cyan wash for checked rows (reading as "selected", and pairing
// with the cyan checkbox), a neutral wash for unchecked. `neutralFill` forces neutral in both
// states for sections that already carry their own colour (Logs Severity).
const fillClass = computed(() => {
  if (props.neutralFill) {
    return props.checked ? 'bg-foreground/[0.09] border-foreground/[0.18]' : 'bg-foreground/[0.05] border-foreground/[0.12]'
  }
  return props.checked ? 'bg-brand/[0.13] border-brand/[0.28]' : 'bg-foreground/[0.05] border-foreground/[0.12]'
})
</script>

<template>
  <div
    :data-test="dataTest"
    role="button"
    tabindex="0"
    :aria-pressed="checked"
    class="group relative isolate flex cursor-pointer items-center gap-2 overflow-hidden rounded-sm px-1.5 py-1 transition-colors motion-reduce:transition-none hover:bg-muted/60 focus-within:bg-muted/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    @click="emit('toggle')"
    @keydown.enter.prevent="emit('toggle')"
    @keydown.space.prevent="emit('toggle')"
  >
    <!-- Marker: defaults to the standard Checkbox; the Severity section swaps in a coloured dot. -->
    <slot name="marker" :checked="checked">
      <Checkbox :model-value="checked" tabindex="-1" />
    </slot>

    <span
      :class="
        cn(
          'flex-1 truncate text-xs',
          mono && 'font-mono',
          labelClass || (checked ? 'text-foreground' : 'text-foreground/60'),
        )
      "
    >
      {{ label }}
    </span>

    <!-- Count fades out on hover/focus to make room for the Only button, which is absolutely
         positioned so it never reserves layout width that would squeeze the label (the
         truncation bug this design fixes). -->
    <span
      v-if="count !== null"
      data-test="facet-count"
      class="shrink-0 text-[10px] tabular-nums text-muted-foreground transition-opacity motion-reduce:transition-none group-hover:opacity-0 group-focus-within:opacity-0"
    >
      {{ formatNumber(count) }}
    </span>

    <button
      type="button"
      :data-test="`${dataTest}-only`"
      title="Only"
      :aria-label="`Only ${label}`"
      class="absolute right-1.5 top-1/2 grid size-5 -translate-y-1/2 place-items-center rounded-sm text-muted-foreground opacity-0 transition-opacity motion-reduce:transition-none hover:bg-accent hover:text-accent-foreground focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring group-hover:opacity-100 group-focus-within:opacity-100"
      @click.stop="emit('only')"
    >
      <Equal class="size-3" />
    </button>

    <!-- Track-fill meter: the value's share fills the row as a soft tint BEHIND the text. `isolate`
         on the row scopes this `-z-10` layer to sit under the label/count but above the row's hover
         background, so the fill can never crowd the text the way the old 2px baseline underline did.
         A faint brand tint marks a checked (selected) row; neutral otherwise; a hairline right edge
         marks the fill boundary. The count still always renders, so a dominant value can't swallow
         the rest. -->
    <span
      v-if="showMeter"
      data-test="facet-meter"
      aria-hidden="true"
      class="pointer-events-none absolute inset-y-0 left-0 -z-10 rounded-sm border-r"
      :class="fillClass"
      :style="{ width: meterWidth }"
    />
  </div>
</template>
