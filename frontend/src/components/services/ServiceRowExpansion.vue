<script setup lang="ts">
// What an expanded row reveals: one child row per Site, then a three-panel mini-dashboard.
//
// This is what replaces a separate "compare mode". The question a Site column is really asked —
// "is this service broken everywhere, or only in Frankfurt?" — is answered by unfolding the row it
// belongs to, in place, without leaving the board.
//
// The panels fetch ONLY while expanded (`enabled`), so a thirty-service board costs nothing until
// somebody actually asks about one service.
import { computed } from 'vue'
import { Badge } from '@/components/ui/badge'
import { Spinner } from '@/components/ui/spinner'
import HealthPill from './HealthPill.vue'
import { useServiceOperations, useServiceDependencies } from '@/lib/services/servicesQueries'
import { formatCompact, formatDuration } from '@/lib/core/format'
import type { FleetRow, SiteSlice } from '@/lib/services/servicesOverview'
import type { RedRow } from '@/lib/core/api'
import { OTHER_OPERATION } from '@/lib/services/servicesOverview'

const props = defineProps<{
  row: FleetRow
  startNs: string
  endNs: string
  /** Column count of the parent table, so the expansion spans the full width. */
  colspan: number
}>()
const emit = defineEmits<{ 'open-site': [slice: SiteSlice]; 'open-operation': [operation: string] }>()

const service = computed(() => props.row.service)
const ops = useServiceOperations(service, () => props.startNs, () => props.endNs, true)
const deps = useServiceDependencies(service, () => props.startNs, () => props.endNs, true)

const PANEL_ROWS = 5

const byRequests = computed<RedRow[]>(() =>
  [...(ops.data.value ?? [])].sort((a, b) => (b.count ?? 0) - (a.count ?? 0)).slice(0, PANEL_ROWS),
)
// Slowest is ranked on p99 but filtered to operations that actually served something: an operation
// with two samples can post any p99 it likes, and putting that at the top of a "slowest" panel
// sends people chasing noise.
const slowest = computed<RedRow[]>(() => {
  const min = Math.max(5, (props.row.count || 0) * 0.001)
  return [...(ops.data.value ?? [])]
    .filter((r) => (r.count ?? 0) >= min)
    .sort((a, b) => Number(b.p99 ?? 0) - Number(a.p99 ?? 0))
    .slice(0, PANEL_ROWS)
})

interface DepRow { name?: string; system?: string; count?: number; error_rate?: number; p99?: string }
// Databases and external calls in one list: from this row's point of view they are all "things
// this service waits on", and splitting them here would give two panels of two rows each.
const worstDeps = computed<DepRow[]>(() => {
  const all = [...(deps.data.value?.database ?? []), ...(deps.data.value?.external ?? [])] as DepRow[]
  return all
    .sort((a, b) => (b.error_rate ?? 0) - (a.error_rate ?? 0) || Number(b.p99 ?? 0) - Number(a.p99 ?? 0))
    .slice(0, PANEL_ROWS)
})

const loading = computed(() => ops.isPending.value || deps.isPending.value)

function opLabel(r: RedRow): string {
  return r.operation ?? '—'
}
/** `__other__` is the rollup's remainder bucket, not an endpoint — it is never clickable. */
function isOther(r: RedRow): boolean {
  return r.operation === OTHER_OPERATION
}
function rateText(r: number | undefined): string {
  const n = r ?? 0
  return (n >= 100 ? formatCompact(Math.round(n)) : n.toFixed(1)) + '/s'
}
</script>

<template>
  <tr data-testid="row-expansion" :data-service="row.service">
    <td :colspan="colspan" class="bg-muted/30 p-0">
      <!-- Per-Site child rows. Dimmed, same column vocabulary as the parent. -->
      <div v-if="row.sites.length > 1" class="border-b border-border/60 px-4 py-2">
        <p class="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">By site</p>
        <div
          v-for="slice in row.sites"
          :key="slice.tenant ?? 'local'"
          data-testid="site-child-row"
          :data-site="slice.tenant ?? 'local'"
          :data-level="slice.level"
          role="button"
          tabindex="0"
          class="grid cursor-pointer grid-cols-[9rem_1fr_5rem_5rem_5rem] items-center gap-3 rounded-md px-2 py-1 font-mono text-[11px] hover:bg-card focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
          @click.stop="emit('open-site', slice)"
          @keydown.enter.stop.prevent="emit('open-site', slice)"
        >
          <span class="flex items-center gap-1.5 truncate">
            <HealthPill :level="slice.level" :show-label="false" />
            <span class="truncate text-foreground/80">{{ slice.tenant ?? 'local' }}</span>
          </span>
          <!-- A stale Site renders em-dashes, never zeros: "0/s, 0% errors, OK" is a claim, and we
               have no data to back it. -->
          <span class="truncate text-muted-foreground">
            {{ slice.siteStale ? 'No recent data from this site' : '' }}
          </span>
          <span class="text-right tabular-nums text-muted-foreground">
            {{ slice.siteStale ? '—' : rateText(slice.rate) }}
          </span>
          <span
            class="text-right tabular-nums"
            :class="!slice.siteStale && (slice.error_count ?? 0) > 0 ? 'text-sev-error' : 'text-muted-foreground'"
          >
            {{ slice.siteStale ? '—' : ((slice.error_rate ?? 0) * 100).toFixed(2) + '%' }}
          </span>
          <span class="text-right tabular-nums text-foreground/70">
            {{ slice.siteStale ? '—' : formatDuration(Number(slice.p99 ?? 0)) }}
          </span>
        </div>
      </div>

      <!-- Three panels: where the traffic is, where the time goes, what it waits on. -->
      <div class="grid grid-cols-1 gap-px bg-border/60 md:grid-cols-3">
        <section class="bg-muted/30 px-4 py-3">
          <h4 class="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Top operations</h4>
          <Spinner v-if="loading" size="sm">loading…</Spinner>
          <p v-else-if="!byRequests.length" class="text-[11px] text-muted-foreground/70">No operations in range</p>
          <ul v-else class="space-y-1">
            <li
              v-for="r in byRequests"
              :key="opLabel(r)"
              data-testid="panel-operation"
              class="flex items-baseline gap-2 font-mono text-[11px]"
            >
              <button
                v-if="!isOther(r)"
                type="button"
                class="min-w-0 flex-1 truncate text-left text-foreground/80 hover:text-primary hover:underline"
                @click.stop="emit('open-operation', opLabel(r))"
              >
                {{ opLabel(r) }}
              </button>
              <span v-else class="min-w-0 flex-1 truncate text-muted-foreground" title="Everything outside this service's busiest operations, summed. Not an endpoint.">
                other operations
              </span>
              <span class="shrink-0 tabular-nums text-muted-foreground">{{ formatCompact(r.count ?? 0) }}</span>
            </li>
          </ul>
        </section>

        <section class="bg-muted/30 px-4 py-3">
          <h4 class="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Slowest operations</h4>
          <Spinner v-if="loading" size="sm">loading…</Spinner>
          <p v-else-if="!slowest.length" class="text-[11px] text-muted-foreground/70">Not enough samples to rank</p>
          <ul v-else class="space-y-1">
            <li v-for="r in slowest" :key="opLabel(r)" class="flex items-baseline gap-2 font-mono text-[11px]">
              <button
                v-if="!isOther(r)"
                type="button"
                class="min-w-0 flex-1 truncate text-left text-foreground/80 hover:text-primary hover:underline"
                @click.stop="emit('open-operation', opLabel(r))"
              >
                {{ opLabel(r) }}
              </button>
              <span v-else class="min-w-0 flex-1 truncate text-muted-foreground">other operations</span>
              <span class="shrink-0 tabular-nums text-foreground/70">{{ formatDuration(Number(r.p99 ?? 0)) }}</span>
            </li>
          </ul>
        </section>

        <section class="bg-muted/30 px-4 py-3">
          <h4 class="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Worst dependencies</h4>
          <Spinner v-if="loading" size="sm">loading…</Spinner>
          <p v-else-if="!worstDeps.length" class="text-[11px] text-muted-foreground/70">No outbound calls in range</p>
          <ul v-else class="space-y-1">
            <li v-for="d in worstDeps" :key="(d.name ?? '') + (d.system ?? '')" class="flex items-baseline gap-2 font-mono text-[11px]">
              <span class="min-w-0 flex-1 truncate text-foreground/80">{{ d.name ?? d.system ?? '—' }}</span>
              <Badge
                v-if="(d.error_rate ?? 0) > 0"
                variant="error"
                class="shrink-0 px-1 py-0 text-[10px]"
              >{{ ((d.error_rate ?? 0) * 100).toFixed(1) }}%</Badge>
              <span class="shrink-0 tabular-nums text-muted-foreground">{{ formatDuration(Number(d.p99 ?? 0)) }}</span>
            </li>
          </ul>
        </section>
      </div>
    </td>
  </tr>
</template>
