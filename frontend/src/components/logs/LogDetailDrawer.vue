<script setup>
import { computed, ref } from 'vue'
import { PeekDrawer } from '@/components/ui/peek-drawer'
import { SheetTitle } from '@/components/ui/sheet'
import { Button } from '@/components/ui/button'
import { Copy, Check, Waypoints, Plus, Minus, ArrowUpRight } from 'lucide-vue-next'
import SeverityTag from '@/components/logs/SeverityTag.vue'
import { formatFull, severity } from '@/lib/core/format'

const props = defineProps({
  row: { type: Object, default: null },
  open: { type: Boolean, default: false },
  index: { type: Number, default: -1 },
  total: { type: Number, default: 0 },
})

const emit = defineEmits(['update:open', 'view-trace', 'prev', 'next', 'filter-value'])

function viewTrace() {
  if (!props.row?.trace_id) return
  emit('view-trace', {
    traceId: props.row.trace_id,
    timeHintNs: props.row.timestamp != null ? props.row.timestamp.toString() : undefined,
  })
}

// Only ever actually open when there's a row to show — keeps "row === null" graceful
// (no drawer, no crash) regardless of what the caller passes for `open`.
const isOpen = computed(() => props.open && !!props.row)

const copiedMessage = ref(false)
const copiedJson = ref(false)
let messageTimer = null
let jsonTimer = null

// The field grid. Each entry carries both its DISPLAY shape (key + value shown in the grid) and,
// when it makes sense, the GRAMMAR mapping the +/− filter actions emit (`field`/`filterValue`) —
// `service.name` filters as `service`, `severity` as `level` (the level key, not the human label),
// attributes filter by their own name. `trace_id`/`span_id` are `jump` rows (↗ into the trace)
// instead. `timestamp` stays visible but is copy-only (an exact-nanosecond match term is never a
// useful triage filter). The logical list is unchanged from before; the extra keys drive the
// hover actions only.
const fields = computed(() => {
  const r = props.row
  if (!r) return []
  const out = [
    { key: 'timestamp', value: formatFull(r.timestamp) },
    { key: 'service.name', value: r.service, field: 'service', filterValue: r.service },
    { key: 'severity', value: severity(r.severity).label, field: 'level', filterValue: r.severity },
  ]
  if (r.trace_id) out.push({ key: 'trace_id', value: r.trace_id, jump: true })
  if (r.span_id) out.push({ key: 'span_id', value: r.span_id, jump: true })
  for (const [k, v] of Object.entries(r.attributes ?? {}))
    out.push({ key: k, value: String(v), field: k, filterValue: String(v) })
  return out
})

async function copyMessage() {
  if (!props.row) return
  await navigator.clipboard.writeText(props.row.body ?? '')
  copiedMessage.value = true
  clearTimeout(messageTimer)
  messageTimer = setTimeout(() => (copiedMessage.value = false), 1500)
}

async function copyValue(value) {
  await navigator.clipboard.writeText(String(value ?? ''))
}

async function copyJson() {
  if (!props.row) return
  const json = JSON.stringify(
    props.row,
    (_key, value) => (typeof value === 'bigint' ? value.toString() : value),
    2,
  )
  await navigator.clipboard.writeText(json)
  copiedJson.value = true
  clearTimeout(jsonTimer)
  jsonTimer = setTimeout(() => (copiedJson.value = false), 1500)
}

// `+` (filter-in) / `−` (filter-out) on a field row. The parent (LogsView) turns this into a
// `field:value` or `-field:value` query-grammar term.
function emitFilter(f, negate) {
  if (!f.field) return
  emit('filter-value', { field: f.field, value: f.filterValue, negate })
}

// PeekDrawer forwards every non-nav keydown (after its input/modifier guard) here. Log-drawer
// keys: `c` copies the message.
function onShortcut(e) {
  if (e.key === 'c') copyMessage()
}
</script>

<template>
  <PeekDrawer
    :open="isOpen"
    :has-content="!!row"
    :index="index"
    :total="total"
    :width="540"
    @update:open="$emit('update:open', $event)"
    @prev="$emit('prev')"
    @next="$emit('next')"
    @shortcut="onShortcut"
  >
    <template #header>
      <SheetTitle class="flex flex-wrap items-center gap-2 text-sm font-medium">
        <SeverityTag :level="row.severity" />
        <span class="font-mono text-foreground">{{ row.service }}</span>
      </SheetTitle>
      <p class="mt-1 font-mono text-xs text-muted-foreground">{{ formatFull(row.timestamp) }}</p>
      <Button
        v-if="row.trace_id"
        data-testid="view-trace"
        variant="outline"
        size="sm"
        class="mt-2 w-fit"
        @click="viewTrace"
      >
        <Waypoints class="mr-1.5 h-3.5 w-3.5" />
        View trace
      </Button>
    </template>

    <div class="px-6 py-4">
      <!-- Message hero: the signal you lead with. -->
      <div class="group relative">
        <p
          data-testid="log-message"
          class="whitespace-pre-wrap break-words rounded-md bg-muted p-4 pr-11 font-mono text-[15px] leading-relaxed text-foreground"
        >{{ row.body }}</p>
        <button
          type="button"
          data-testid="copy-message"
          :title="copiedMessage ? 'Copied' : 'Copy message (c)'"
          class="absolute right-2 top-2 inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground opacity-0 transition-opacity hover:bg-background hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
          @click="copyMessage"
        >
          <Check v-if="copiedMessage" class="h-4 w-4 text-foreground" />
          <Copy v-else class="h-4 w-4" />
        </button>
      </div>

      <!-- Fields -->
      <div class="mb-2 mt-6 flex items-center justify-between">
        <span class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Fields</span>
        <Button variant="outline" size="sm" @click="copyJson">
          <Check v-if="copiedJson" class="mr-1.5 h-3.5 w-3.5" />
          <Copy v-else class="mr-1.5 h-3.5 w-3.5" />
          {{ copiedJson ? 'Copied' : 'Copy JSON' }}
        </Button>
      </div>
      <div class="text-xs">
        <div
          v-for="f in fields"
          :key="f.key"
          class="group flex items-center gap-3 border-t border-border/60 py-1.5"
        >
          <span class="w-[130px] shrink-0 truncate font-mono text-muted-foreground">{{ f.key }}</span>
          <span class="min-w-0 flex-1 truncate font-mono text-foreground">{{ f.value }}</span>
          <div
            class="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100"
          >
            <template v-if="f.jump">
              <button
                type="button"
                :data-testid="'field-jump-' + f.key"
                title="View trace"
                class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                @click="viewTrace"
              >
                <ArrowUpRight class="h-3.5 w-3.5" />
              </button>
            </template>
            <template v-else-if="f.field">
              <button
                type="button"
                :data-testid="'filter-in-' + f.key"
                title="Filter in"
                class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                @click="emitFilter(f, false)"
              >
                <Plus class="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                :data-testid="'filter-out-' + f.key"
                title="Filter out"
                class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                @click="emitFilter(f, true)"
              >
                <Minus class="h-3.5 w-3.5" />
              </button>
            </template>
            <button
              type="button"
              :data-testid="'copy-value-' + f.key"
              title="Copy value"
              class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
              @click="copyValue(f.value)"
            >
              <Copy class="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      </div>
    </div>
  </PeekDrawer>
</template>
