<script setup lang="ts">
// AlertsView's "channels" tab body: a card grid of notification channels + "+ Add channel",
// mirroring UptimeDashboard's loading/error/empty-state shape. Rule counts per channel come from
// `useRules()` (a rule's `channel_ids` names the channels it notifies) rather than the channel
// object itself, which only carries its own webhook config.
import { computed, ref } from 'vue'
import { Plus } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { useChannels, useRules } from '@/lib/alertsQueries'
import ChannelCard from './ChannelCard.vue'
import ChannelDialog from './ChannelDialog.vue'
import type { AlertChannel } from '@/lib/core/api'

const channelsQuery = useChannels()
const channels = computed(() => channelsQuery.data.value ?? [])
const isLoading = computed(() => channelsQuery.isLoading.value)
const isError = computed(() => channelsQuery.isError.value)

const rulesQuery = useRules()
const ruleCountByChannel = computed(() => {
  const counts: Record<string, number> = {}
  for (const rule of rulesQuery.data.value ?? []) {
    for (const id of rule.channel_ids) counts[id] = (counts[id] ?? 0) + 1
  }
  return counts
})

const showDialog = ref(false)
const editingChannel = ref<AlertChannel | null>(null)

function openCreate() {
  editingChannel.value = null
  showDialog.value = true
}
function openEdit(id: string) {
  editingChannel.value = channels.value.find((c) => c.id === id) ?? null
  showDialog.value = true
}
</script>

<template>
  <div>
    <div class="mb-4 flex justify-end">
      <Button size="sm" @click="openCreate">
        <Plus class="mr-1.5 size-3.5" />
        Add channel
      </Button>
    </div>

    <p v-if="isLoading" class="text-sm text-muted-foreground"><Spinner size="sm">Loading…</Spinner></p>
    <p v-else-if="isError" class="text-sm text-destructive">Failed to load channels.</p>
    <EmptyState
      v-else-if="!channels.length"
      title="No channels yet"
      description="Add a webhook channel so rules have somewhere to notify."
    />
    <div v-else class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
      <ChannelCard
        v-for="c in channels"
        :key="c.id"
        :channel="c"
        :rule-count="ruleCountByChannel[c.id] ?? 0"
        @edit="openEdit"
      />
    </div>

    <ChannelDialog v-model:open="showDialog" :channel="editingChannel" />
  </div>
</template>
