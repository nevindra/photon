<script setup lang="ts">
// The last few actions on this host as a list, so "did my restart land?" is answered on the
// page that asked for it — one row per action, its outcome on the left where a scan lands, the
// agent's own words (a registry's 404, "restarted in 2.1s") under the target. Active ones
// (queued / running) always show; settled ones fade after an hour.
import { computed } from 'vue'
import { useNow } from '@vueuse/core'
import { RouterLink } from 'vue-router'
import { Loader2 } from 'lucide-vue-next'
import { StatusDot } from '@/components/ui/status-dot'
import { StatusPill } from '@/components/ui/status-pill'
import { ACTION_META, ACTION_STATUS_META } from '@/lib/deploy/actions'
import { ageMs, untilMs } from '@/lib/deploy/audit'
import type { ActionRecord } from '@/lib/core/api'

const props = defineProps<{ actions: ActionRecord[]; host: string }>()
// Ticks so "4s ago" moves between polls; a card about things happening right now must move.
const now = useNow({ interval: 1000 })
const shown = computed(() => {
  const cutoff = Date.now() - 60 * 60_000
  return props.actions.filter((a) => a.status === 'queued' || a.status === 'running' || a.created_ms >= cutoff).slice(0, 6)
})
</script>

<template>
  <section v-if="shown.length" class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="host-actions">
    <header class="flex items-center gap-2 border-b border-border px-4 py-2.5">
      <h3 class="text-sm font-medium text-foreground">Recent actions</h3>
      <span class="text-xs text-muted-foreground">what Photon did to this host</span>
      <RouterLink :to="`/audit?host=${encodeURIComponent(host)}`" class="ml-auto text-xs text-muted-foreground hover:text-foreground hover:underline">Audit →</RouterLink>
    </header>
    <ul class="divide-y divide-border/60">
      <li v-for="a in shown" :key="a.id" class="grid grid-cols-[1rem_1fr_auto] items-center gap-3 px-4 py-2 text-xs" :data-action-id="a.id" :data-status="a.status" :title="a.status === 'queued' ? `expires in ${untilMs(a.expires_ms)} unless the host picks it up` : undefined">
        <Loader2 v-if="a.status === 'running'" class="size-3.5 animate-spin text-brand" />
        <StatusDot v-else :tone="ACTION_STATUS_META[a.status].tone" size="md" />
        <div class="min-w-0">
          <div class="flex items-center gap-1.5 text-foreground">
            <span class="font-medium">{{ ACTION_META[a.kind].label }}</span>
            <StatusPill v-if="a.status !== 'done'" :tone="ACTION_STATUS_META[a.status].tone" :dot="false">{{ ACTION_STATUS_META[a.status].label }}</StatusPill>
          </div>
          <div v-if="a.container_name ?? a.image ?? a.volume ?? a.result" class="truncate font-mono text-[11px] text-muted-foreground" :title="a.result ?? undefined">
            {{ a.container_name ?? a.image ?? a.volume }}<template v-if="a.result"><template v-if="a.container_name ?? a.image ?? a.volume"> · </template>{{ a.result }}</template>
          </div>
        </div>
        <span class="whitespace-nowrap text-muted-foreground">{{ a.status === 'done' ? 'Done · ' : '' }}{{ ageMs(a.created_ms, now.getTime()) }}</span>
      </li>
    </ul>
  </section>
</template>
