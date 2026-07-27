<script setup>
import { computed, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { Plus } from 'lucide-vue-next'
import { useStorage } from '@vueuse/core'
import AppShell from '@/components/common/AppShell.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { useMonitors, useCreateMonitor } from '@/lib/uptime/uptimeQueries'
import UptimeStatBand from '@/components/uptime/UptimeStatBand.vue'
import MonitorTable from '@/components/uptime/MonitorTable.vue'
import MonitorCard from '@/components/uptime/MonitorCard.vue'
import MonitorForm from '@/components/uptime/MonitorForm.vue'
import MonitorDetailDialog from '@/components/uptime/MonitorDetailDialog.vue'

const route = useRoute()

const monitorsQuery = useMonitors()
const allMonitors = computed(() => monitorsQuery.data.value ?? [])
const isLoading = computed(() => monitorsQuery.isLoading.value)
const isError = computed(() => monitorsQuery.isError.value)

// Free-text monitor filter, matched case-insensitively against a monitor's name AND its target.
// It doubles as the landing spot for the service → Uptime pivot (`/uptime?q=<service>`, see
// lib/core/useCorrelate.ts): a Monitor has no service field — only `name` and `target` — so a
// name/target match is the honest best-effort link between the two, NOT a modeled relationship.
// Kept in sync with `?q=` so the filtered view is shareable and survives a reload.
const filterText = ref(typeof route.query.q === 'string' ? route.query.q : '')
watch(
  () => route.query.q,
  (q) => {
    if (typeof q === 'string' && q !== filterText.value) filterText.value = q
  },
)
watch(filterText, (q) => {
  if (typeof window === 'undefined') return
  const params = new URLSearchParams(window.location.search)
  if (q) params.set('q', q)
  else params.delete('q')
  const qs = params.toString()
  window.history.replaceState(null, '', qs ? `?${qs}` : window.location.pathname)
})

const monitors = computed(() => {
  const q = filterText.value.trim().toLowerCase()
  if (!q) return allMonitors.value
  return allMonitors.value.filter(
    (m) =>
      (m.name ?? '').toLowerCase().includes(q) || (m.target ?? '').toLowerCase().includes(q),
  )
})

const view = useStorage('photon.uptime.view', 'table') // 'table' | 'cards'
const selectedId = ref(null)
const showCreate = ref(false)
const create = useCreateMonitor()

function openDetail(id) {
  selectedId.value = id
}
function onCreate(body) {
  create.mutate(body)
}
function setView(v) {
  // reka-ui's single-select toggle group deselects (emits undefined) when the
  // active item is clicked again — ignore that so a view is always selected.
  if (!v) return
  view.value = v
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Ops">
    <section class="p-6">
      <header class="mb-6 flex items-center justify-between gap-4">
        <div>
          <h1 class="text-xl font-semibold text-foreground">Uptime</h1>
          <p class="text-sm text-muted-foreground">Monitor HTTP(S), TCP and ping targets.</p>
        </div>
        <div class="flex items-center gap-2">
          <Input
            v-model="filterText"
            data-testid="uptime-filter"
            type="text"
            placeholder="Filter monitors…"
            aria-label="Filter monitors by name or target"
            class="h-8 w-52 font-mono text-xs"
          />
          <Segmented :model-value="view" @update:model-value="setView">
            <SegmentedItem v-for="opt in ['table', 'cards']" :key="opt" :value="opt" class="capitalize">
              {{ opt }}
            </SegmentedItem>
          </Segmented>
          <Button size="sm" @click="showCreate = true">
            <Plus class="mr-1.5 size-3.5" />
            Add Monitor
          </Button>
        </div>
      </header>

      <p v-if="isLoading" class="text-sm text-muted-foreground"><Spinner size="sm">Loading…</Spinner></p>
      <p v-else-if="isError" class="text-sm text-destructive">Failed to load monitors.</p>
      <EmptyState v-else-if="!allMonitors.length" title="No monitors yet" description="Add your first one." />
      <EmptyState
        v-else-if="!monitors.length"
        data-testid="uptime-no-matches"
        title="No matching monitors"
        :description="`No monitor's name or target contains “${filterText.trim()}”.`"
      />
      <template v-else>
        <UptimeStatBand :monitors="monitors" class="mb-5" />
        <MonitorTable v-if="view === 'table'" :monitors="monitors" @select="openDetail" />
        <div v-else class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          <MonitorCard v-for="m in monitors" :key="m.id" :monitor="m" @select="openDetail" />
        </div>
      </template>

      <MonitorForm v-model="showCreate" @save="onCreate" />
      <MonitorDetailDialog
        :monitor-id="selectedId"
        :open="!!selectedId"
        @update:open="(v) => { if (!v) selectedId = null }"
      />
    </section>
  </AppShell>
</template>
