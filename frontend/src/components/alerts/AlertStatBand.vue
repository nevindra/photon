<script setup lang="ts">
// AlertsView's stat band. Unlike UptimeStatBand (which receives `monitors` as a prop from its one
// parent view), this one self-fetches: AlertsView renders it once above the tab bar, so it can't
// depend on whichever tab body happens to be mounted for its data. All three composables poll on
// their own ~15s interval and are cache-deduped against the identical queries the Rules/Incidents/
// Channels tab bodies mount, so this doesn't add extra network traffic beyond the first paint.
import { computed } from 'vue'
import { StatTile } from '@/components/ui/stat-tile'
import { useRules, useChannels, useIncidents } from '@/lib/alertsQueries'

const rulesQuery = useRules()
const channelsQuery = useChannels()
const triggeredQuery = useIncidents({ status: 'triggered' })

const rules = computed(() => rulesQuery.data.value ?? [])
const activeRules = computed(() => rules.value.filter((r) => r.enabled).length)
const paused = computed(() => rules.value.filter((r) => !r.enabled).length)
const triggered = computed(() => triggeredQuery.data.value?.length ?? 0)
const channelCount = computed(() => channelsQuery.data.value?.length ?? 0)

type Accent = 'success' | 'error' | 'warning' | 'info' | 'neutral'
const tiles = computed<Array<{ key: string; label: string; n: number; accent?: Accent }>>(() => [
  { key: 'triggered', label: 'Triggered', n: triggered.value, accent: 'error' },
  { key: 'active', label: 'Active rules', n: activeRules.value, accent: 'success' },
  { key: 'paused', label: 'Paused', n: paused.value, accent: 'neutral' },
  { key: 'channels', label: 'Channels', n: channelCount.value },
])
</script>

<template>
  <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
    <StatTile v-for="t in tiles" :key="t.key" :label="t.label" :value="t.n" :accent="t.accent" />
  </div>
</template>
