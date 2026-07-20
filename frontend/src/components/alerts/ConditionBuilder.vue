<script setup lang="ts">
// The sentence-style condition builder used inside AlertRuleDialog (Task 14). A signal segmented
// control (Metrics/Logs/Traces/RUM) swaps the field set; the rest reads as plain English with the
// blanks as dropdowns/inputs, mirroring the approved mockup
// (.superpowers/brainstorm/37613-1784341076/content/alerts-final.html). `for` (sustained-breach
// duration) is deliberately NOT here — per the design doc §5, `for_secs` is a column of
// `alert_rules`, not part of the per-signal `condition` JSON, so AlertRuleDialog owns that field.
//
// Ownership model: this component seeds its local `form`/`signal` state ONCE from the initial
// `condition` prop at setup time (no ongoing prop watcher) and only ever WRITES upward via
// `update:condition` — never reads its own emitted value back. That sidesteps the classic
// controlled-textbox round-trip bug (e.g. trimming a string on emit would otherwise snap back and
// strip a trailing space the user just typed). AlertRuleDialog re-seeds a fresh instance by
// `:key`-ing this component on the target rule's id, exactly like MonitorForm.vue reseeds via a
// prop watcher — same effect, safe for a component that must also bubble every keystroke upward
// (unlike MonitorForm, which only emits on submit).
import { computed, reactive, ref, watch } from 'vue'
import { refDebounced } from '@vueuse/core'
import { X } from 'lucide-vue-next'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { SelectMenu } from '@/components/ui/select-menu'
import { Input } from '@/components/ui/input'
import { NumberField } from '@/components/ui/number-field'
import { FormField } from '@/components/ui/form-field'
import { StatusPill } from '@/components/ui/status-pill'
import SearchBar from '@/components/common/SearchBar.vue'
import { signalColor } from '@/lib/core/signalMeta'
import { startNs, endNs } from '@/lib/core/context'
import { useMetricCatalog } from '@/lib/metrics/metricsQueries'
import { useServices } from '@/lib/logs/logsQueries'
import { useRumApps } from '@/lib/rum/rumQueries'
import { usePreview } from '@/lib/alertsQueries'
import type {
  AlertCondition,
  AlertSignal,
  AlertCmp,
  MetricsCondition,
  TracesCondition,
  RumCondition,
  AlertPreviewSeries,
} from '@/lib/core/api'

const props = defineProps<{ condition: AlertCondition | null }>()
const emit = defineEmits<{ (e: 'update:condition', value: AlertCondition): void }>()

const uid = Math.random().toString(36).slice(2, 8)

// --- Signal picker -------------------------------------------------------------------------

const SIGNALS: { value: AlertSignal; label: string }[] = [
  { value: 'metrics', label: 'Metrics' },
  { value: 'logs', label: 'Logs' },
  { value: 'traces', label: 'Traces' },
  { value: 'rum', label: 'RUM' },
]

// --- Shared option lists ---------------------------------------------------------------------

const DURATIONS = [
  { value: 60, label: '1m' },
  { value: 300, label: '5m' },
  { value: 600, label: '10m' },
  { value: 900, label: '15m' },
  { value: 1800, label: '30m' },
  { value: 3600, label: '1h' },
]
const CMP_OPTIONS: { value: AlertCmp; label: string }[] = [
  { value: 'gt', label: 'above' },
  { value: 'gte', label: 'at least' },
  { value: 'lt', label: 'below' },
  { value: 'lte', label: 'at most' },
]
const AGG_OPTIONS: { value: MetricsCondition['agg']; label: string }[] = [
  { value: 'avg', label: 'avg' },
  { value: 'min', label: 'min' },
  { value: 'max', label: 'max' },
  { value: 'sum', label: 'sum' },
  { value: 'last', label: 'last' },
  { value: 'p50', label: 'p50' },
  { value: 'p90', label: 'p90' },
  { value: 'p99', label: 'p99' },
  { value: 'rate', label: 'rate' },
  { value: 'increase', label: 'increase' },
]
const LOGS_GROUP_BY_OPTIONS = [
  { value: '', label: 'no grouping' },
  { value: 'service.name', label: 'per service.name' },
]
const TRACES_KIND_OPTIONS: { value: TracesCondition['kind']; label: string }[] = [
  { value: 'error_rate', label: 'error rate' },
  { value: 'latency_p50', label: 'latency p50' },
  { value: 'latency_p90', label: 'latency p90' },
  { value: 'latency_p99', label: 'latency p99' },
  { value: 'request_rate', label: 'request rate' },
]
const RUM_KIND_OPTIONS: { value: RumCondition['kind']; label: string }[] = [
  { value: 'vital_lcp_p75', label: 'LCP p75' },
  { value: 'vital_inp_p75', label: 'INP p75' },
  { value: 'vital_cls_p75', label: 'CLS p75' },
  { value: 'vital_fcp_p75', label: 'FCP p75' },
  { value: 'vital_ttfb_p75', label: 'TTFB p75' },
  { value: 'error_count', label: 'error count' },
]

// --- Local form state ------------------------------------------------------------------------

interface FormState {
  metric_name: string
  label_filters: Record<string, string>
  group_by: string[]
  agg: MetricsCondition['agg']
  query: string
  logsGroupBy: string // '' = no grouping, else 'service.name'
  service: string
  tracesKind: TracesCondition['kind']
  app_id: string
  route: string
  rumKind: RumCondition['kind']
  window_secs: number
  cmp: AlertCmp
  threshold: number
}

const SIGNAL_DEFAULTS: Record<AlertSignal, Partial<FormState>> = {
  metrics: { window_secs: 300, cmp: 'gt', threshold: 0.9, agg: 'avg' },
  logs: { window_secs: 600, cmp: 'gt', threshold: 100 },
  traces: { window_secs: 300, cmp: 'gt', threshold: 5, tracesKind: 'error_rate' },
  rum: { window_secs: 900, cmp: 'gt', threshold: 2500, rumKind: 'vital_lcp_p75' },
}

function blankForm(): FormState {
  return {
    metric_name: '',
    label_filters: {},
    group_by: [],
    agg: 'avg',
    query: '',
    logsGroupBy: '',
    service: '',
    tracesKind: 'error_rate',
    app_id: '',
    route: '',
    rumKind: 'vital_lcp_p75',
    window_secs: 300,
    cmp: 'gt',
    threshold: 0,
  }
}

function seed(c: AlertCondition | null): { signal: AlertSignal; form: FormState } {
  const sig = c?.signal ?? 'metrics'
  const form = { ...blankForm(), ...SIGNAL_DEFAULTS[sig] }
  if (c) {
    switch (c.signal) {
      case 'metrics':
        form.metric_name = c.metric_name
        form.label_filters = c.label_filters ? { ...c.label_filters } : {}
        form.group_by = c.group_by ? [...c.group_by] : []
        form.agg = c.agg
        break
      case 'logs':
        form.query = c.query
        form.logsGroupBy = c.group_by ?? ''
        break
      case 'traces':
        form.service = c.service
        form.tracesKind = c.kind
        break
      case 'rum':
        form.app_id = c.app_id
        form.route = c.route ?? ''
        form.rumKind = c.kind
        break
    }
    form.window_secs = c.window_secs
    form.cmp = c.cmp
    form.threshold = c.threshold
  }
  return { signal: sig, form }
}

const initial = seed(props.condition)
const signal = ref<AlertSignal>(initial.signal)
const form = reactive<FormState>(initial.form)

function onPickSignal(v: unknown) {
  if (typeof v !== 'string' || !v || v === signal.value) return
  signal.value = v as AlertSignal
  Object.assign(form, blankForm(), SIGNAL_DEFAULTS[signal.value])
}

// --- Group-by chips (metrics only) -----------------------------------------------------------

const groupByDraft = ref('')
function addGroupBy() {
  const v = groupByDraft.value.trim()
  if (v && !form.group_by.includes(v)) form.group_by.push(v)
  groupByDraft.value = ''
}
function onGroupByKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    addGroupBy()
  }
}
function removeGroupBy(i: number) {
  form.group_by.splice(i, 1)
}

// --- Label-filter chips (metrics only) -------------------------------------------------------

const labelFilterDraft = ref('')
function addLabelFilter() {
  const raw = labelFilterDraft.value.trim()
  const eq = raw.indexOf('=')
  if (eq > 0) {
    const k = raw.slice(0, eq).trim()
    const v = raw.slice(eq + 1).trim()
    if (k && v) form.label_filters[k] = v
  }
  labelFilterDraft.value = ''
}
function onLabelFilterKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    addLabelFilter()
  }
}
function removeLabelFilter(k: string) {
  delete form.label_filters[k]
}

// --- Autocomplete sources ---------------------------------------------------------------------

const catalogQuery = useMetricCatalog(startNs, endNs)
const metricNames = computed(() => catalogQuery.data.value?.map((m) => m.name) ?? [])
const servicesQuery = useServices()
const serviceNames = computed(() => servicesQuery.data.value ?? [])
const rumAppsQuery = useRumApps()
const rumAppNames = computed(() => (rumAppsQuery.data.value?.apps ?? []).map((a) => a.name))

// --- Unit hints + threshold precision ----------------------------------------------------------

const unitLabel = computed(() => {
  if (signal.value === 'traces') {
    if (form.tracesKind === 'error_rate') return '%'
    if (form.tracesKind === 'request_rate') return 'req/s'
    return 'ms'
  }
  if (signal.value === 'rum') {
    if (form.rumKind === 'error_count') return 'errors'
    if (form.rumKind === 'vital_cls_p75') return ''
    return 'ms'
  }
  if (signal.value === 'logs') return 'matches'
  return ''
})
const thresholdStep = computed(() => {
  if (signal.value === 'traces' && form.tracesKind === 'error_rate') return 0.1
  if (signal.value === 'rum' && form.rumKind === 'vital_cls_p75') return 0.01
  if (signal.value === 'metrics') return 0.01
  return 1
})

// --- Build + emit the Condition (design doc §5.1 shapes, exactly) ------------------------------

const builtCondition = computed<AlertCondition>(() => {
  const shared = { window_secs: form.window_secs, cmp: form.cmp, threshold: form.threshold }
  switch (signal.value) {
    case 'metrics':
      return {
        signal: 'metrics',
        metric_name: form.metric_name,
        label_filters: Object.keys(form.label_filters).length ? { ...form.label_filters } : undefined,
        agg: form.agg,
        group_by: form.group_by.length ? [...form.group_by] : undefined,
        ...shared,
      }
    case 'logs':
      return {
        signal: 'logs',
        query: form.query,
        group_by: form.logsGroupBy || null,
        ...shared,
      }
    case 'traces':
      return {
        signal: 'traces',
        service: form.service,
        kind: form.tracesKind,
        ...shared,
      }
    case 'rum':
      return {
        signal: 'rum',
        app_id: form.app_id,
        route: form.route.trim() ? form.route.trim() : null,
        kind: form.rumKind,
        ...shared,
      }
  }
})

const isValid = computed(() => {
  switch (signal.value) {
    case 'metrics':
      return form.metric_name.trim().length > 0
    case 'logs':
      return form.query.trim().length > 0
    case 'traces':
      return form.service.trim().length > 0
    case 'rum':
      return form.app_id.trim().length > 0
    default:
      return false
  }
})

watch(builtCondition, (c) => emit('update:condition', c), { immediate: true })

// --- Live preview: debounced POST /api/alerts/preview -------------------------------------------

const previewCondition = ref<AlertCondition | null>(null)
watch(
  [builtCondition, isValid],
  () => {
    previewCondition.value = isValid.value ? builtCondition.value : null
  },
  { immediate: true },
)
const debouncedCondition = refDebounced(previewCondition, 400)
const previewQuery = usePreview(debouncedCondition)
const previewSeries = computed<AlertPreviewSeries[]>(() => previewQuery.data.value?.series ?? [])
const breachingSeries = computed(() => previewSeries.value.filter((s) => s.breaching))

function seriesLabel(s: AlertPreviewSeries): string {
  const entries = Object.entries(s.series_key ?? {})
  if (!entries.length) return 'aggregate'
  return entries.map(([k, v]) => `${k}=${v}`).join(', ')
}
const breachingLabels = computed(() => breachingSeries.value.slice(0, 3).map(seriesLabel))

// Exposed so AlertRuleDialog can gate its Create/Save button and power the "Test now" fallback
// for an unsaved draft (a saved rule tests itself server-side via useTestRule instead).
defineExpose({ isValid, previewSeries })
</script>

<template>
  <div class="space-y-4">
    <Segmented :model-value="signal" @update:model-value="onPickSignal">
      <SegmentedItem v-for="s in SIGNALS" :key="s.value" :value="s.value">
        <span class="size-2 rounded-[2px]" :style="{ backgroundColor: signalColor(s.value) }" />
        {{ s.label }}
      </SegmentedItem>
    </Segmented>

    <div class="rounded-lg border border-border bg-muted/40 p-4">
      <!-- Metrics -->
      <div v-if="signal === 'metrics'" class="flex flex-wrap items-center gap-x-2 gap-y-2 text-[15px] leading-relaxed">
        <span class="text-muted-foreground">Alert when</span>
        <SelectMenu v-model="form.agg" :options="AGG_OPTIONS" content-class="w-28" aria-label="Aggregation" />
        <span class="text-muted-foreground">of</span>
        <Input
          v-model="form.metric_name"
          :list="`metric-catalog-${uid}`"
          placeholder="metric name…"
          class="h-8 w-52 font-mono text-xs"
          autocomplete="off"
        />
        <datalist :id="`metric-catalog-${uid}`">
          <option v-for="m in metricNames" :key="m" :value="m" />
        </datalist>
        <span class="text-muted-foreground">over</span>
        <SelectMenu v-model="form.window_secs" :options="DURATIONS" content-class="w-24" aria-label="Window" />
        <span class="text-muted-foreground">is</span>
        <SelectMenu v-model="form.cmp" :options="CMP_OPTIONS" content-class="w-28" aria-label="Comparison" />
        <NumberField v-model="form.threshold" :step="thresholdStep" class="w-24" :show-steppers="false" />
      </div>

      <!-- Logs -->
      <div v-else-if="signal === 'logs'" class="flex flex-wrap items-center gap-x-2 gap-y-2 text-[15px] leading-relaxed">
        <span class="text-muted-foreground">Alert when count of logs matching</span>
      </div>
      <div v-if="signal === 'logs'" class="mt-2">
        <SearchBar v-model="form.query" :services="serviceNames" placeholder="severity:error service.name:payments" />
      </div>
      <div v-if="signal === 'logs'" class="mt-3 flex flex-wrap items-center gap-x-2 gap-y-2 text-[15px] leading-relaxed">
        <span class="text-muted-foreground">over</span>
        <SelectMenu v-model="form.window_secs" :options="DURATIONS" content-class="w-24" aria-label="Window" />
        <span class="text-muted-foreground">is</span>
        <SelectMenu v-model="form.cmp" :options="CMP_OPTIONS" content-class="w-28" aria-label="Comparison" />
        <NumberField v-model="form.threshold" :step="thresholdStep" class="w-24" :show-steppers="false" />
        <span class="text-xs text-muted-foreground">{{ unitLabel }}</span>
        <span class="text-muted-foreground">·</span>
        <SelectMenu v-model="form.logsGroupBy" :options="LOGS_GROUP_BY_OPTIONS" content-class="w-40" aria-label="Group by" />
      </div>

      <!-- Traces -->
      <div v-else-if="signal === 'traces'" class="flex flex-wrap items-center gap-x-2 gap-y-2 text-[15px] leading-relaxed">
        <span class="text-muted-foreground">Alert when</span>
        <SelectMenu v-model="form.tracesKind" :options="TRACES_KIND_OPTIONS" content-class="w-36" aria-label="Metric kind" />
        <span class="text-muted-foreground">of service</span>
        <Input
          v-model="form.service"
          :list="`service-catalog-${uid}`"
          placeholder="checkout-api"
          class="h-8 w-44 font-mono text-xs"
          autocomplete="off"
        />
        <datalist :id="`service-catalog-${uid}`">
          <option v-for="s in serviceNames" :key="s" :value="s" />
        </datalist>
        <span class="text-muted-foreground">over</span>
        <SelectMenu v-model="form.window_secs" :options="DURATIONS" content-class="w-24" aria-label="Window" />
        <span class="text-muted-foreground">is</span>
        <SelectMenu v-model="form.cmp" :options="CMP_OPTIONS" content-class="w-28" aria-label="Comparison" />
        <NumberField v-model="form.threshold" :step="thresholdStep" class="w-24" :show-steppers="false" />
        <span class="text-xs text-muted-foreground">{{ unitLabel }}</span>
      </div>

      <!-- RUM -->
      <template v-else-if="signal === 'rum'">
        <div class="flex flex-wrap items-center gap-x-2 gap-y-2 text-[15px] leading-relaxed">
          <span class="text-muted-foreground">Alert when</span>
          <SelectMenu v-model="form.rumKind" :options="RUM_KIND_OPTIONS" content-class="w-32" aria-label="Vital" />
          <span class="text-muted-foreground">of app</span>
          <Input
            v-model="form.app_id"
            :list="`rum-app-catalog-${uid}`"
            placeholder="storefront"
            class="h-8 w-40 font-mono text-xs"
            autocomplete="off"
          />
          <datalist :id="`rum-app-catalog-${uid}`">
            <option v-for="a in rumAppNames" :key="a" :value="a" />
          </datalist>
          <span class="text-muted-foreground">over</span>
          <SelectMenu v-model="form.window_secs" :options="DURATIONS" content-class="w-24" aria-label="Window" />
          <span class="text-muted-foreground">is</span>
          <SelectMenu v-model="form.cmp" :options="CMP_OPTIONS" content-class="w-28" aria-label="Comparison" />
          <NumberField v-model="form.threshold" :step="thresholdStep" class="w-24" :show-steppers="false" />
          <span class="text-xs text-muted-foreground">{{ unitLabel }}</span>
        </div>
        <FormField label="Route" for="rum-route" :optional="true" class="mt-3">
          <Input id="rum-route" v-model="form.route" placeholder="e.g. /checkout (leave empty for all routes)" autocomplete="off" />
        </FormField>
      </template>

      <!-- Label-filter chips (metrics only) -->
      <FormField
        v-if="signal === 'metrics'"
        label="Filter labels"
        :optional="true"
        hint="Restrict to matching label values, e.g. host.name=web-01."
        class="mt-3"
      >
        <div class="flex flex-wrap items-center gap-1.5">
          <span
            v-for="(v, k) in form.label_filters"
            :key="k"
            class="inline-flex items-center gap-1 rounded-md border border-border bg-muted px-2 py-1 text-xs font-mono"
          >
            {{ k }}={{ v }}
            <button type="button" class="text-muted-foreground hover:text-foreground" @click="removeLabelFilter(k)">
              <X class="size-3" />
            </button>
          </span>
          <input
            v-model="labelFilterDraft"
            placeholder="host.name=web-01…"
            class="h-7 w-40 rounded-md border border-input bg-background px-2 font-mono text-xs outline-none focus-visible:ring-1 focus-visible:ring-ring"
            @keydown="onLabelFilterKeydown"
            @blur="addLabelFilter"
          >
        </div>
      </FormField>

      <!-- Group-by chips (metrics only) -->
      <FormField
        v-if="signal === 'metrics'"
        label="Group by"
        :optional="true"
        hint="Each combination becomes its own series with independent trigger/resolve."
        class="mt-3"
      >
        <div class="flex flex-wrap items-center gap-1.5">
          <span
            v-for="(g, i) in form.group_by"
            :key="g"
            class="inline-flex items-center gap-1 rounded-md border border-border bg-muted px-2 py-1 text-xs font-mono"
          >
            {{ g }}
            <button type="button" class="text-muted-foreground hover:text-foreground" @click="removeGroupBy(i)">
              <X class="size-3" />
            </button>
          </span>
          <input
            v-model="groupByDraft"
            placeholder="host.name…"
            class="h-7 w-32 rounded-md border border-input bg-background px-2 font-mono text-xs outline-none focus-visible:ring-1 focus-visible:ring-ring"
            @keydown="onGroupByKeydown"
            @blur="addGroupBy"
          >
        </div>
      </FormField>

      <!-- Live preview -->
      <div class="mt-4">
        <StatusPill v-if="!debouncedCondition" tone="neutral">
          Fill in the condition to preview
        </StatusPill>
        <StatusPill v-else-if="previewQuery.isFetching.value" tone="neutral">
          Checking current data…
        </StatusPill>
        <StatusPill v-else-if="previewQuery.isError.value" tone="neutral">
          Couldn't evaluate the preview
        </StatusPill>
        <StatusPill v-else-if="breachingSeries.length === 0" tone="success">
          Calm — no series breaching right now
        </StatusPill>
        <StatusPill v-else tone="error">
          Will trigger on {{ breachingSeries.length }} series now{{ breachingLabels.length ? ` (${breachingLabels.join(', ')}${breachingSeries.length > breachingLabels.length ? ', …' : ''})` : '' }}
        </StatusPill>
      </div>
    </div>
  </div>
</template>
