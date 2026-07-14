<script setup>
import { computed, ref, watch } from 'vue'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import StatePill from '@/components/uptime/StatePill.vue'
import HeartbeatBar from '@/components/uptime/HeartbeatBar.vue'
import ResponseTimeChart from '@/components/uptime/ResponseTimeChart.vue'
import MonitorForm from '@/components/uptime/MonitorForm.vue'
import { Card } from '@/components/ui/card'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Spinner } from '@/components/ui/spinner'
import {
  useMonitor,
  useHeartbeats,
  useIncidents,
  useUpdateMonitor,
  useDeleteMonitor,
  usePauseMonitor,
  useResumeMonitor,
} from '@/lib/uptime/uptimeQueries'

const props = defineProps({
  monitorId: { type: String, default: null },
  open: { type: Boolean, default: false },
})
const emit = defineEmits(['update:open'])

const id = computed(() => props.monitorId)
const window_ = ref('24h')
const monitorQ = useMonitor(id)
const monitor = computed(() => monitorQ.data.value)
const isLoading = computed(() => monitorQ.isLoading.value)
const hbQ = useHeartbeats(id, window_)
const hb = computed(() => hbQ.data.value)
const incidentsQ = useIncidents(id)
const incidents = computed(() => incidentsQ.data.value)
const update = useUpdateMonitor()
const del = useDeleteMonitor()
const pause = usePauseMonitor()
const resume = useResumeMonitor()

const showEdit = ref(false)

const WINDOWS = ['24h', '7d', '30d']

watch(id, () => {
  window_.value = '24h'
})

function onSave(body) {
  update.mutate({ id: id.value, body })
}
function togglePause() {
  monitor.value?.enabled ? pause.mutate(id.value) : resume.mutate(id.value)
}
function setWindow(w) {
  // reka-ui's single-select toggle group deselects (emits undefined) when the
  // active item is clicked again — ignore that so a window is always selected.
  if (!w) return
  window_.value = w
}
function onDelete() {
  del.mutate(id.value)
  emit('update:open', false)
}
function fmt(ts) {
  return new Date(ts).toLocaleString()
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-w-2xl max-h-[85vh] overflow-y-auto">
      <DialogHeader>
        <div class="flex flex-wrap items-center gap-3">
          <DialogTitle>{{ monitor?.name ?? 'Monitor' }}</DialogTitle>
          <template v-if="monitor">
            <StatePill :state="monitor.last_state" :paused="!monitor.enabled" />
            <span
              class="rounded-full border border-border px-2 py-0.5 text-xs uppercase tracking-wide text-muted-foreground"
            >
              {{ monitor.type }}
            </span>
          </template>
        </div>
        <DialogDescription v-if="monitor" class="font-mono">{{ monitor.target }}</DialogDescription>
      </DialogHeader>

      <template v-if="monitor">
        <!-- Uptime + window switch -->
        <div class="flex items-end justify-between">
          <div>
            <div class="text-3xl font-semibold text-foreground">
              {{ (hb?.uptime_pct ?? 100).toFixed(2) }}%
            </div>
            <div class="text-xs text-muted-foreground">uptime · {{ window_ }}</div>
          </div>
          <Segmented :model-value="window_" @update:model-value="setWindow" class="text-xs">
            <SegmentedItem v-for="w in WINDOWS" :key="w" :value="w">{{ w }}</SegmentedItem>
          </Segmented>
        </div>

        <HeartbeatBar :heartbeats="hb?.heartbeats ?? []" size="lg" show-legend />

        <!-- Response time -->
        <ResponseTimeChart :heartbeats="hb?.heartbeats ?? []" />

        <!-- Incidents -->
        <Card class="p-3">
          <div class="mb-2 text-sm font-medium text-foreground">Incidents</div>
          <div v-if="!incidents?.length" class="text-sm text-muted-foreground">No incidents 🎉</div>
          <ul v-else class="space-y-2 text-sm">
            <li v-for="i in incidents" :key="i.id" class="flex flex-col gap-0.5">
              <span class="text-sev-error">{{ i.cause }}</span>
              <span class="text-xs text-muted-foreground">
                {{ fmt(i.started_at) }} → {{ i.ended_at ? fmt(i.ended_at) : 'ongoing' }}
              </span>
            </li>
          </ul>
        </Card>

        <DialogFooter>
          <button
            type="button"
            class="rounded-md border border-input px-3 py-1.5 text-sm text-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
            @click="togglePause"
          >
            {{ monitor.enabled ? 'Pause' : 'Resume' }}
          </button>
          <button
            type="button"
            class="rounded-md border border-input px-3 py-1.5 text-sm text-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
            @click="showEdit = true"
          >
            Edit
          </button>
          <button
            type="button"
            class="rounded-md border border-sev-error/50 px-3 py-1.5 text-sm text-sev-error transition-colors hover:bg-sev-error-soft"
            @click="onDelete"
          >
            Delete
          </button>
        </DialogFooter>

        <MonitorForm v-model="showEdit" :monitor="monitor" @save="onSave" />
      </template>
      <p v-else class="text-sm text-muted-foreground"><Spinner size="sm">Loading…</Spinner></p>
    </DialogContent>
  </Dialog>
</template>
