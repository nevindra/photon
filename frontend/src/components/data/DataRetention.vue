<script setup>
// The Retention tab of the /data page: one numeric input per signal, saved as a partial update,
// now card-framed with a per-signal "window used" gauge (how much of the configured retention
// window the oldest surviving data already occupies). `useRetention`/`useSetRetention` and the
// save/validation logic are unchanged from the pre-revamp version — seed the local form from the
// query the first time each key arrives (don't clobber in-progress edits on a background
// refetch); only the markup and the gauge below are new.
import { reactive, ref, computed, watch, onUnmounted } from 'vue'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { NumberField } from '@/components/ui/number-field'
import { Spinner } from '@/components/ui/spinner'
import { useRetention, useSetRetention, useStorage } from '@/lib/data/dataQueries'
import { formatNumber } from '@/lib/core/format'
import { signalColor, signalIcon } from '@/lib/core/signalMeta'

const { data: retentionData, isLoading: retentionLoading } = useRetention()
const setRetention = useSetRetention()
const { data: storage } = useStorage()

const form = reactive({})
watch(
  retentionData,
  (r) => {
    if (!r) return
    for (const [k, v] of Object.entries(r)) if (form[k] === undefined) form[k] = v
  },
  { immediate: true },
)
const retentionSignals = computed(() => (retentionData.value ? Object.keys(retentionData.value) : []))

const retentionError = ref('')
const retentionSaved = ref(false)
const savingRetention = ref(false)
let savedTimer

async function saveRetention() {
  if (savingRetention.value) return
  retentionError.value = ''
  retentionSaved.value = false
  const partial = {}
  for (const k of retentionSignals.value) {
    const v = Number(form[k])
    if (!Number.isInteger(v) || v <= 0) {
      retentionError.value = `Retention for ${k} must be a whole number of days greater than 0.`
      return
    }
    partial[k] = v
  }
  savingRetention.value = true
  try {
    const res = await setRetention.mutateAsync(partial)
    if (res && res.ok === false) {
      retentionError.value = res.error || 'Could not save retention.'
      return
    }
    retentionSaved.value = true
    clearTimeout(savedTimer)
    savedTimer = setTimeout(() => (retentionSaved.value = false), 3000)
  } finally {
    savingRetention.value = false
  }
}

onUnmounted(() => clearTimeout(savedTimer))

// ---- "Window used" gauge -------------------------------------------------------------------
// Age (in days) of the oldest surviving row for `sig`, or null when there's no data yet (so the
// gauge stays hidden rather than drawing a meaningless bar). Parquet signals report
// `min_ts_nanos` (epoch ns), but `StorageStats` defaults every field to 0 when the manifest is
// empty rather than leaving the timestamp null — so an empty signal is detected via `file_count`
// (mirroring DataStorage.vue's own "No data" check), not a null check on the timestamp. Uptime
// instead reports `oldest_heartbeat_ts` (epoch ms) as a true optional: null means no heartbeats
// recorded yet, which we can check directly.
function oldestAgeDays(sig) {
  const stat = storage.value?.signals?.[sig]
  if (!stat) return null
  if ('oldest_heartbeat_ts' in stat) {
    if (stat.oldest_heartbeat_ts == null) return null
    return (Date.now() - Number(stat.oldest_heartbeat_ts)) / 86_400_000
  }
  if (!stat.file_count || stat.min_ts_nanos == null) return null
  return (Date.now() - Number(stat.min_ts_nanos) / 1e6) / 86_400_000
}

// Fraction of the configured retention window the oldest surviving data already occupies. >= 1
// means the oldest row is already older than the configured window (it's due to be dropped by the
// next compaction sweep). Guards against a not-yet-seeded/blank `form[sig]` by reporting 0 rather
// than NaN/Infinity.
function retentionRatio(sig) {
  const age = oldestAgeDays(sig)
  const days = Number(form[sig])
  if (age == null || !days) return 0
  return age / days
}
</script>

<template>
  <form
    class="flex flex-col gap-4 rounded-xl border border-border bg-card p-4 shadow-1"
    @submit.prevent="saveRetention"
  >
    <p class="text-xs text-muted-foreground">
      How long to keep each signal before it is automatically deleted.
    </p>

    <Spinner v-if="retentionLoading" size="sm">Loading…</Spinner>
    <template v-else>
      <div class="flex flex-col">
        <div
          v-for="sig in retentionSignals"
          :key="sig"
          class="grid grid-cols-1 gap-2 border-b border-border py-3 first:pt-0 last:border-0 last:pb-0 sm:grid-cols-[10rem_9rem_1fr] sm:items-center sm:gap-4"
        >
          <div class="flex items-center gap-2">
            <span
              class="flex size-7 shrink-0 items-center justify-center rounded-md"
              :style="{ background: signalColor(sig) + '1a' }"
            >
              <component :is="signalIcon(sig)" class="size-4" :style="{ color: signalColor(sig) }" />
            </span>
            <span class="text-sm font-medium capitalize text-card-foreground">{{ sig }}</span>
          </div>

          <div class="flex items-center gap-2">
            <Label :for="`ret-${sig}`" class="sr-only">{{ sig }} retention, in days</Label>
            <NumberField :id="`ret-${sig}`" v-model="form[sig]" :min="1" :show-steppers="false" class="w-24" />
            <span class="text-xs text-muted-foreground">days</span>
          </div>

          <div v-if="oldestAgeDays(sig) != null" class="flex min-w-0 flex-col gap-1">
            <div class="h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                class="h-full rounded-full"
                :class="retentionRatio(sig) >= 1 ? 'bg-sev-warn' : ''"
                :style="{
                  width: Math.min(1, retentionRatio(sig)) * 100 + '%',
                  background: retentionRatio(sig) >= 1 ? undefined : signalColor(sig),
                }"
              />
            </div>
            <div class="flex items-center justify-between gap-2 text-[11px]">
              <span class="text-muted-foreground">
                Oldest data is {{ formatNumber(Math.round(oldestAgeDays(sig))) }}d old
              </span>
              <span :class="retentionRatio(sig) >= 1 ? 'text-sev-warn' : 'text-muted-foreground'">
                {{ retentionRatio(sig) >= 1 ? 'over limit' : `${Math.round(retentionRatio(sig) * 100)}%` }}
              </span>
            </div>
          </div>
          <p v-else class="text-[11px] text-muted-foreground">No data yet.</p>
        </div>
      </div>

      <p v-if="retentionError" class="font-mono text-xs text-sev-error">{{ retentionError }}</p>
      <p v-else-if="retentionSaved" class="text-xs text-muted-foreground">Saved.</p>

      <div class="flex flex-wrap items-center justify-between gap-3 border-t border-border pt-3">
        <p class="text-xs text-muted-foreground">Changes apply on the next compaction sweep.</p>
        <Button type="submit" :disabled="savingRetention">
          {{ savingRetention ? 'Saving…' : 'Save' }}
        </Button>
      </div>
    </template>
  </form>
</template>
