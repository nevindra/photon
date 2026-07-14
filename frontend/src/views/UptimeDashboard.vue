<script setup>
import { computed, ref } from 'vue'
import { Plus } from 'lucide-vue-next'
import { useStorage } from '@vueuse/core'
import AppShell from '@/components/common/AppShell.vue'
import { Button } from '@/components/ui/button'
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

const monitorsQuery = useMonitors()
const monitors = computed(() => monitorsQuery.data.value ?? [])
const isLoading = computed(() => monitorsQuery.isLoading.value)
const isError = computed(() => monitorsQuery.isError.value)

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
      <EmptyState v-else-if="!monitors.length" title="No monitors yet" description="Add your first one." />
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
