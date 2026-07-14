<script setup>
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { StatusDot } from '@/components/ui/status-dot'
import { X, FileText, Copy } from 'lucide-vue-next'
import { formatFull, formatDuration } from '@/lib/core/format'
import { pct } from '@/lib/traces/traceTree'
import { serviceColorClass } from '@/lib/services/serviceColor'
import { cn } from '@/lib/core/utils'
import { useCopy } from '@/lib/core/useCopy'
import RelatedMenu from '@/components/common/RelatedMenu.vue'

const props = defineProps({
  span: { type: Object, default: null },
  traceId: { type: String, default: '' },
  // Built tree nodes (see lib/traceTree.js `buildTrace`), used only to render the "in context"
  // mini-waterfall and self-time on the Overview tab. Optional — the panel degrades gracefully
  // (omits both) when the caller doesn't have tree context handy.
  node: { type: Object, default: null },
  parentNode: { type: Object, default: null },
})
const emit = defineEmits(['close', 'view-logs'])

const TABS = [
  { id: 'overview', label: 'Overview' },
  { id: 'attributes', label: 'Attributes' },
  { id: 'events', label: 'Events' },
  { id: 'raw', label: 'Raw' },
]
const activeTab = ref('overview')
// DOM refs for the tab buttons, keyed by tab id — used to move focus for roving tabindex.
const tabRefs = {}
function setTabRef(id, el) {
  if (el) tabRefs[id] = el
}

// Values the user has expanded to full wrap (vs. the default truncate) on the
// Attributes/Identity rows, keyed by attribute/identity key.
const expanded = reactive(new Set())
function toggleExpand(key) {
  if (expanded.has(key)) expanded.delete(key)
  else expanded.add(key)
}

// A new span means stale tab/expand state from the previous span shouldn't carry over.
watch(
  () => props.span?.span_id,
  () => {
    activeTab.value = 'overview'
    expanded.clear()
  },
)

function focusTab(id) {
  activeTab.value = id
  nextTick(() => tabRefs[id]?.focus())
}

function onTablistKeydown(e) {
  const idx = TABS.findIndex((t) => t.id === activeTab.value)
  if (e.key === 'ArrowRight') focusTab(TABS[(idx + 1) % TABS.length].id)
  else if (e.key === 'ArrowLeft') focusTab(TABS[(idx - 1 + TABS.length) % TABS.length].id)
  else if (e.key === 'Home') focusTab(TABS[0].id)
  else if (e.key === 'End') focusTab(TABS[TABS.length - 1].id)
  else return
  e.preventDefault()
}

const isError = computed(() => props.span?.status_code === 2)

// Cross-signal jump-off for this span (logs for span/trace, backend health, similar traces),
// each hop carrying the active time window + scope via correlate().
const relatedEntity = computed(() => ({
  kind: 'span',
  fields: {
    traceId: props.span?.trace_id ?? props.traceId,
    spanId: props.span?.span_id,
    service: props.span?.service,
    operation: props.span?.name,
  },
}))

const timing = computed(() => {
  const s = props.span
  if (!s) return []
  const out = [
    ['start', formatFull(s.start_time_nanos)],
    ['duration', formatDuration(s.duration_nanos)],
  ]
  if (props.node) out.push(['self', formatDuration(props.node.selfTimeNs)])
  if (s.end_time_nanos != null) out.push(['end', formatFull(s.end_time_nanos)])
  return out
})

const identity = computed(() => {
  const s = props.span
  if (!s) return []
  const out = [['span_id', s.span_id]]
  if (s.parent_span_id) out.push(['parent_span_id', s.parent_span_id])
  out.push(['kind', s.kind_text ?? String(s.kind ?? '')])
  if (s.scope_name) out.push(['scope_name', s.scope_name])
  out.push(['trace_id', s.trace_id ?? props.traceId])
  return out
})

const attributes = computed(() => Object.entries(props.span?.attributes ?? {}))
const events = computed(() => (Array.isArray(props.span?.events) ? props.span.events : []))
const links = computed(() => (Array.isArray(props.span?.links) ? props.span.links : []))

// "In context" mini-waterfall rows: parent (if given) → this span → its children. Purely
// presentational — geometry comes straight off the node/parentNode the caller passes in.
const miniRows = computed(() => {
  const rows = []
  if (props.parentNode) {
    rows.push({
      id: 'parent',
      label: props.parentNode.span?.name ?? 'parent',
      service: props.parentNode.span?.service,
      startNs: props.parentNode.startNs,
      endNs: props.parentNode.endNs,
      isError: props.parentNode.span?.status_code === 2,
      isCurrent: false,
    })
  }
  if (props.node) {
    const curSpan = props.node.span ?? props.span
    rows.push({
      id: 'current',
      label: curSpan?.name ?? 'this span',
      service: curSpan?.service,
      startNs: props.node.startNs,
      endNs: props.node.endNs,
      isError: curSpan?.status_code === 2,
      isCurrent: true,
    })
    for (const c of props.node.children ?? []) {
      rows.push({
        id: c.span?.span_id ?? `child-${rows.length}`,
        label: c.span?.name ?? '',
        service: c.span?.service,
        startNs: c.startNs,
        endNs: c.endNs,
        isError: !!c.isError,
        isCurrent: false,
      })
    }
  }
  return rows
})

const miniRange = computed(() => {
  const rows = miniRows.value
  if (!rows.length) return null
  let start = rows[0].startNs
  let end = rows[0].endNs
  for (const r of rows) {
    if (r.startNs != null && r.startNs < start) start = r.startNs
    if (r.endNs != null && r.endNs > end) end = r.endNs
  }
  return { start, total: end - start }
})

function miniBarStyle(row) {
  const range = miniRange.value
  if (!range || row.startNs == null || row.endNs == null) return { left: '0%', width: '2%' }
  const left = pct(row.startNs - range.start, range.total)
  const width = Math.max(pct(row.endNs - row.startNs, range.total), 2)
  return { left: `${left}%`, width: `${Math.min(width, 100 - left)}%` }
}

// BigInt-safe JSON: spans carry BigInt start/end nanos, which JSON.stringify throws on
// without a replacer.
const rawJson = computed(() => {
  if (!props.span) return ''
  return JSON.stringify(props.span, (_k, v) => (typeof v === 'bigint' ? v.toString() : v), 2)
})

function eventOffset(e) {
  const s = props.span
  if (!s || e?.time_unix_nano == null || s.start_time_nanos == null) return null
  const t = typeof e.time_unix_nano === 'bigint' ? e.time_unix_nano : BigInt(e.time_unix_nano)
  const start =
    typeof s.start_time_nanos === 'bigint' ? s.start_time_nanos : BigInt(s.start_time_nanos)
  return t - start
}

function viewLogs() {
  const s = props.span
  emit('view-logs', { query: `trace_id:${props.traceId} span_id:${s.span_id}` })
}

const { copy } = useCopy()

async function copySpanId() {
  if (props.span?.span_id) await copy(props.span.span_id, 'span ID')
}

async function copyRaw() {
  if (rawJson.value) await copy(rawJson.value, 'span JSON')
}
</script>

<template>
  <aside
    v-if="span"
    data-testid="span-panel"
    class="flex w-[360px] shrink-0 flex-col border-l border-border bg-background"
  >
    <div class="flex items-start justify-between gap-2 border-b border-border px-4 py-3">
      <div class="min-w-0">
        <div class="flex items-center gap-2">
          <StatusDot :tone="isError ? 'error' : 'neutral'" class="size-2" />
          <span class="truncate font-mono text-sm text-foreground">{{ span.name }}</span>
        </div>
        <span class="font-mono text-xs text-muted-foreground">{{ span.service }}</span>
      </div>
      <div class="flex shrink-0 items-center gap-1.5">
        <RelatedMenu :entity="relatedEntity" />
        <button
          type="button"
          data-testid="close-panel"
          class="rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-accent-foreground"
          aria-label="Close span detail"
          @click="emit('close')"
        >
          <X class="size-4" />
        </button>
      </div>
    </div>

    <!-- Always-visible error callout: renders regardless of the active tab. -->
    <Alert v-if="isError" variant="error" class="border-b border-t-0">
      <AlertDescription class="font-mono text-xs">
        {{ span.status_text ?? span.status_code }}<template v-if="span.status_message"> — {{ span.status_message }}</template>
      </AlertDescription>
    </Alert>

    <div role="tablist" class="flex shrink-0 gap-1 border-b border-border px-2" @keydown="onTablistKeydown">
      <button
        v-for="t in TABS"
        :key="t.id"
        :ref="(el) => setTabRef(t.id, el)"
        type="button"
        role="tab"
        :data-test="`tab-${t.id}`"
        :aria-selected="activeTab === t.id"
        :tabindex="activeTab === t.id ? 0 : -1"
        :class="
          cn(
            'border-b-2 px-2.5 py-2 text-[10px] font-medium uppercase tracking-wider',
            activeTab === t.id
              ? 'border-foreground text-foreground'
              : 'border-transparent text-muted-foreground hover:text-foreground',
          )
        "
        @click="activeTab = t.id"
      >
        {{ t.label }}
      </button>
    </div>

    <ScrollArea class="min-h-0 flex-1">
      <div role="tabpanel" :data-test="`tabpanel-${activeTab}`" class="space-y-5 px-4 py-4">
        <template v-if="activeTab === 'overview'">
          <section v-if="miniRows.length" data-testid="mini-waterfall">
            <p class="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">In context</p>
            <div class="space-y-1">
              <div v-for="row in miniRows" :key="row.id" class="flex items-center gap-2">
                <span :class="cn('size-1.5 shrink-0 rounded-full', serviceColorClass(row.service))" />
                <span
                  :class="
                    cn(
                      'w-20 shrink-0 truncate font-mono text-[10px]',
                      row.isCurrent ? 'text-foreground' : 'text-muted-foreground',
                    )
                  "
                  >{{ row.label }}</span
                >
                <div class="relative h-3 flex-1 rounded-sm bg-muted">
                  <span
                    :class="
                      cn(
                        'absolute inset-y-0 rounded-sm',
                        row.isError ? 'bg-sev-error' : row.isCurrent ? 'bg-foreground/70' : 'bg-muted-foreground/50',
                      )
                    "
                    :style="miniBarStyle(row)"
                  />
                </div>
              </div>
            </div>
          </section>

          <section>
            <p class="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Timing</p>
            <dl class="grid grid-cols-[90px_1fr] gap-x-3 text-xs">
              <template v-for="[k, v] in timing" :key="k">
                <dt class="border-t border-border/60 py-1.5 font-mono text-muted-foreground">{{ k }}</dt>
                <dd class="border-t border-border/60 py-1.5 font-mono text-foreground">{{ v }}</dd>
              </template>
            </dl>
          </section>

          <section>
            <p class="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Identity</p>
            <dl class="grid grid-cols-[110px_1fr] gap-x-3 text-xs">
              <template v-for="[k, v] in identity" :key="k">
                <dt class="truncate border-t border-border/60 py-1.5 font-mono text-muted-foreground">{{ k }}</dt>
                <dd class="border-t border-border/60 py-1.5 font-mono text-foreground">
                  <div class="group flex items-start gap-1">
                    <button
                      type="button"
                      :class="expanded.has(k) ? 'whitespace-pre-wrap break-all text-left' : 'truncate text-left'"
                      @click="toggleExpand(k)"
                    >{{ v }}</button>
                    <button
                      type="button"
                      :data-test="`attr-copy-${k}`"
                      class="shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100 hover:text-foreground"
                      aria-label="Copy value"
                      @click.stop="copy(v, k)"
                    >
                      <Copy class="size-3" />
                    </button>
                  </div>
                </dd>
              </template>
            </dl>
          </section>

          <section v-if="links.length">
            <p class="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Links</p>
            <ul class="space-y-1">
              <li v-for="(l, i) in links" :key="i" class="truncate rounded bg-muted p-2 font-mono text-xs text-foreground">
                {{ l.trace_id }}<template v-if="l.span_id"> / {{ l.span_id }}</template>
              </li>
            </ul>
          </section>
        </template>

        <template v-else-if="activeTab === 'attributes'">
          <section v-if="attributes.length">
            <dl class="grid grid-cols-[130px_1fr] gap-x-3 text-xs">
              <template v-for="[k, v] in attributes" :key="k">
                <dt class="truncate border-t border-border/60 py-1.5 font-mono text-muted-foreground">{{ k }}</dt>
                <dd class="border-t border-border/60 py-1.5 font-mono text-foreground">
                  <div class="group flex items-start gap-1">
                    <button
                      type="button"
                      :class="expanded.has(k) ? 'whitespace-pre-wrap break-all text-left' : 'truncate text-left'"
                      @click="toggleExpand(k)"
                    >{{ v }}</button>
                    <button
                      type="button"
                      :data-test="`attr-copy-${k}`"
                      class="shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100 hover:text-foreground"
                      aria-label="Copy value"
                      @click.stop="copy(v, k)"
                    >
                      <Copy class="size-3" />
                    </button>
                  </div>
                </dd>
              </template>
            </dl>
          </section>
          <p v-else class="font-mono text-xs text-muted-foreground">No attributes.</p>
        </template>

        <template v-else-if="activeTab === 'events'">
          <ul v-if="events.length" class="space-y-1">
            <li v-for="(e, i) in events" :key="i" class="flex items-center justify-between gap-2 rounded bg-muted p-2 font-mono text-xs text-foreground">
              <span class="truncate">◆ {{ e.name }}</span>
              <span class="shrink-0 text-muted-foreground">+{{ formatDuration(eventOffset(e)) }}</span>
            </li>
          </ul>
          <p v-else class="font-mono text-xs text-muted-foreground">No events.</p>
        </template>

        <template v-else-if="activeTab === 'raw'">
          <div class="space-y-2">
            <div class="flex justify-end">
              <Button variant="outline" size="sm" @click="copyRaw">
                <Copy class="mr-2 size-3.5" />
                Copy JSON
              </Button>
            </div>
            <pre class="overflow-x-auto rounded bg-muted p-2 font-mono text-[11px] text-foreground">{{ rawJson }}</pre>
          </div>
        </template>
      </div>
    </ScrollArea>

    <div class="flex flex-col gap-2 border-t border-border px-4 py-3">
      <Button data-testid="view-logs" variant="default" size="sm" class="w-full justify-start" @click="viewLogs">
        <FileText class="mr-2 size-3.5" />
        View logs for this span
      </Button>
      <Button variant="outline" size="sm" class="w-full justify-start" @click="copySpanId">
        <Copy class="mr-2 size-3.5" />
        Copy span ID
      </Button>
    </div>
  </aside>
</template>
