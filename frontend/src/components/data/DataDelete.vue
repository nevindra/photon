<script setup>
// The Delete tab of the /data page: per-signal purge actions — "delete older than <date>" plus a
// "delete all" behind a type-to-confirm gate. Lifted from the old SettingsData delete section; the
// only change is the card source, which now reads `storage.signals` (the reshaped payload) instead
// of the flat top-level object. Per-signal UI state is keyed by signal name and seeded as each
// signal appears in storage.
import { reactive, computed, watch, onUnmounted } from 'vue'
import { TriangleAlert } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Card } from '@/components/ui/card'
import { Spinner } from '@/components/ui/spinner'
import { DatePicker } from '@/components/ui/date-picker'
import { formatNumber, formatBytes } from '@/lib/core/format'
import { signalColor, signalIcon } from '@/lib/core/signalMeta'
import { useStorage, usePurge } from '@/lib/data/dataQueries'

const { data: storage, isLoading: storageLoading } = useStorage()
const purge = usePurge()

const signals = computed(() => (storage.value?.signals ? Object.keys(storage.value.signals) : []))

const deleteDate = reactive({})
const confirmText = reactive({})
const purgeBusy = reactive({})
const purgeStatus = reactive({})
const purgeError = reactive({})
const clearTimers = {}

watch(
  signals,
  (keys) => {
    for (const key of keys) {
      if (deleteDate[key] === undefined) deleteDate[key] = ''
      if (confirmText[key] === undefined) confirmText[key] = ''
      if (purgeBusy[key] === undefined) purgeBusy[key] = false
      if (purgeStatus[key] === undefined) purgeStatus[key] = ''
      if (purgeError[key] === undefined) purgeError[key] = ''
    }
  },
  { immediate: true },
)

// Per-signal count line under the header: rows/bytes for Parquet signals, heartbeats for uptime.
function countLabel(signal) {
  const stat = storage.value?.signals?.[signal]
  if (!stat) return ''
  if ('monitor_count' in stat) return `${formatNumber(stat.heartbeat_count ?? 0)} heartbeats`
  return `${formatNumber(stat.total_rows ?? 0)} rows · ${formatBytes(stat.bytes)}`
}

// Render whatever the purge report carries — Parquet (files/rows) or uptime (heartbeats/incidents).
function formatReport(report) {
  if (!report) return 'Purge complete.'
  const parts = []
  if (report.files_removed != null) parts.push(`${formatNumber(report.files_removed)} files`)
  if (report.rows_removed != null) parts.push(`${formatNumber(report.rows_removed)} rows`)
  if (report.heartbeats_removed != null) parts.push(`${formatNumber(report.heartbeats_removed)} heartbeats`)
  if (report.incidents_removed != null) parts.push(`${formatNumber(report.incidents_removed)} incidents`)
  return parts.length ? `Removed ${parts.join(', ')}.` : 'Purge complete.'
}

function scheduleClear(signal) {
  clearTimeout(clearTimers[signal])
  clearTimers[signal] = setTimeout(() => (purgeStatus[signal] = ''), 6000)
}

async function runPurge(signal, body) {
  if (purgeBusy[signal]) return
  purgeError[signal] = ''
  purgeStatus[signal] = ''
  purgeBusy[signal] = true
  try {
    const res = await purge.mutateAsync(body)
    if (res && res.ok === false) {
      purgeError[signal] = res.error || 'Purge failed.'
      return null
    }
    purgeStatus[signal] = formatReport(res?.report)
    scheduleClear(signal)
    return res
  } finally {
    purgeBusy[signal] = false
  }
}

async function purgeBefore(signal) {
  const dateStr = deleteDate[signal]
  if (!dateStr) {
    purgeError[signal] = 'Pick a date first.'
    return
  }
  const before_ms = Date.parse(dateStr)
  if (Number.isNaN(before_ms)) {
    purgeError[signal] = 'Invalid date.'
    return
  }
  await runPurge(signal, { signal, mode: 'before', before_ms })
}

async function purgeAll(signal) {
  if (confirmText[signal] !== 'DELETE') return
  const res = await runPurge(signal, { signal, mode: 'all' })
  if (res) confirmText[signal] = ''
}

onUnmounted(() => {
  for (const t of Object.values(clearTimers)) clearTimeout(t)
})
</script>

<template>
  <Spinner v-if="storageLoading" size="sm">Loading…</Spinner>
  <div v-else class="flex flex-col gap-4">
    <div class="flex items-start gap-2 rounded-lg border border-sev-error/40 bg-sev-error-soft px-4 py-3 text-xs">
      <TriangleAlert class="mt-0.5 size-4 shrink-0 text-sev-error" />
      <p class="text-sev-error/90">
        Purges are <span class="font-semibold">immediate and permanent</span>. Deleting data here also
        removes any durable (replicated) copies — there is no undo.
      </p>
    </div>

    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
      <Card v-for="signal in signals" :key="signal" class="flex flex-col gap-3 rounded-xl p-4">
        <div class="flex items-center justify-between gap-2">
          <div class="flex items-center gap-2">
            <span
              class="flex size-6 shrink-0 items-center justify-center rounded-md"
              :style="{ color: signalColor(signal), background: signalColor(signal) + '22' }"
            >
              <component :is="signalIcon(signal)" class="size-3.5" />
            </span>
            <p class="text-sm font-medium capitalize text-card-foreground">{{ signal }}</p>
          </div>
          <p class="font-mono text-[11px] text-muted-foreground">{{ countLabel(signal) }}</p>
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <Label class="text-xs text-muted-foreground">Delete older than</Label>
          <DatePicker v-model="deleteDate[signal]" />
          <Button variant="outline" size="sm" :disabled="purgeBusy[signal]" @click="purgeBefore(signal)">
            Delete range
          </Button>
        </div>

        <div class="flex items-center gap-2 text-[10px] uppercase tracking-wider text-muted-foreground">
          <span class="h-px flex-1 bg-border" />
          or
          <span class="h-px flex-1 bg-border" />
        </div>

        <div class="flex flex-wrap items-center gap-2">
          <Input
            v-model="confirmText[signal]"
            type="text"
            placeholder="type DELETE to confirm"
            autocomplete="off"
            class="w-52"
            :aria-label="`Type DELETE to confirm deleting all ${signal} data`"
          />
          <Button
            variant="destructive"
            size="sm"
            :disabled="confirmText[signal] !== 'DELETE' || purgeBusy[signal]"
            @click="purgeAll(signal)"
          >
            Delete all {{ signal }}
          </Button>
        </div>

        <p v-if="purgeError[signal]" class="font-mono text-xs text-sev-error">
          {{ purgeError[signal] }}
        </p>
        <p v-else-if="purgeStatus[signal]" class="font-mono text-xs text-muted-foreground">
          {{ purgeStatus[signal] }}
        </p>
      </Card>
    </div>
  </div>
</template>
