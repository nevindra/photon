<script setup lang="ts">
// The fleet as one table: every host, whichever source describes it, ranked worst first. A row
// says WHY it ranks where it does in words ("disk 90%", "worst of 3 mounts · average 61%") beside
// its four meters, and what the host is (site · OS · arch, from the agent's Docker facts when it
// has one). A host with no metrics — waiting for its first sync, or an agent that went quiet — is
// a row too, greyed, its status in the place the meters would be; the page's count is the list's.
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { Container, Search } from 'lucide-vue-next'
import { Input } from '@/components/ui/input'
import { Meter } from '@/components/ui/meter'
import { StatusDot } from '@/components/ui/status-dot'
import { StatusPill } from '@/components/ui/status-pill'
import { Badge } from '@/components/ui/badge'
import { EmptyState } from '@/components/ui/empty-state'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { relative } from '@/lib/core/format'
import { formatPct, utilAccent } from '@/lib/infra/hostStats'
import { ageMs } from '@/lib/deploy/audit'
import { AGENT_STATUS_META } from '@/lib/deploy/agentStatus'
import { LEVEL_META, type FleetRow, type FleetLevel } from '@/lib/infra/fleet'
import { cn } from '@/lib/core/utils'

const props = defineProps<{
  rows: FleetRow[]
  filter: FleetLevel | null
  query: string
}>()
const emit = defineEmits<{ select: [host: string]; 'update:query': [string] }>()

const shown = computed(() => {
  const q = props.query.trim().toLowerCase()
  return props.rows.filter((r) => {
    if (props.filter && r.level !== props.filter) return false
    if (!q) return true
    const a = r.agent
    return r.host.toLowerCase().includes(q) || (a?.site ?? '').toLowerCase().includes(q) || Object.entries(a?.labels ?? {}).some(([k, v]) => `${k}=${v}`.toLowerCase().includes(q) || v.toLowerCase().includes(q))
  })
})

interface Cell { value: number; mean: number | null }
function cell(value: number | null, mean: number | null, groups: number): Cell | null {
  return value == null ? null : { value, mean: groups > 1 ? mean : null }
}
function meters(r: FleetRow): { cpu: Cell | null; mem: Cell | null; disk: Cell | null; gpu: Cell | null } {
  const m = r.metrics
  if (!m) return { cpu: null, mem: null, disk: null, gpu: null }
  return { cpu: cell(m.cpuUtil, null, 1), mem: cell(m.memUtil, null, 1), disk: cell(m.diskUtil, m.diskUtilAvg, m.diskGroups), gpu: cell(m.gpuUtil, m.gpuUtilAvg, m.gpuGroups) }
}
function tone(c: Cell): 'error' | 'warning' | 'info' {
  return utilAccent(c.value) ?? 'info'
}
function describe(r: FleetRow): string {
  const parts: string[] = []
  if (r.agent?.site) parts.push(r.agent.site)
  else if (r.agent) parts.push('local')
  if (r.agent?.docker) parts.push(r.agent.docker.os, r.agent.docker.arch)
  return parts.join(' · ')
}
</script>

<template>
  <section class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="infra-fleet-table">
    <header class="flex flex-wrap items-center gap-2 px-3 py-2">
      <span class="text-xs text-muted-foreground">Sorted by attention</span>
      <div class="relative ml-auto w-64 max-w-full">
        <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input :model-value="query" placeholder="Filter by name, site, label…" class="h-8 pl-8 text-xs" data-testid="fleet-query" @update:model-value="emit('update:query', String($event))" />
      </div>
    </header>
    <Table v-if="shown.length" container-class="overflow-x-auto border-t border-border" class="text-sm">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead class="w-6" />
          <TableHead>Host</TableHead>
          <TableHead>Why</TableHead>
          <TableHead class="w-32">CPU</TableHead>
          <TableHead class="w-32">Memory</TableHead>
          <TableHead class="w-36">Disk</TableHead>
          <TableHead class="w-32">GPU</TableHead>
          <TableHead class="w-24 text-right">Containers</TableHead>
          <TableHead class="w-28">Agent</TableHead>
          <TableHead class="w-20 text-right">Seen</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="r in shown"
          :key="r.host"
          data-testid="fleet-row"
          :data-host="r.host"
          :data-level="r.level"
          :class="cn('cursor-pointer border-border/60', (r.level === 'pending' || r.level === 'offline') && !r.metrics && 'bg-muted/40 text-muted-foreground')"
          @click="emit('select', r.host)"
        >
          <TableCell class="py-2.5 pr-0"><StatusDot :tone="LEVEL_META[r.level].tone" size="lg" :title="LEVEL_META[r.level].label" /></TableCell>
          <TableCell class="max-w-[16rem] py-2.5">
            <!-- The name is the row's one real control (keyboard + screen reader); the row click is
                 a pointer convenience on top. -->
            <button type="button" class="block max-w-full truncate text-left font-medium text-foreground hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring" @click.stop="emit('select', r.host)">{{ r.host }}</button>
            <div v-if="describe(r)" class="truncate text-[11px] text-muted-foreground" :title="describe(r)">{{ describe(r) }}</div>
          </TableCell>

          <!-- No metrics: the status says why there are no meters, across their columns. -->
          <template v-if="!r.metrics">
            <TableCell colspan="5" class="py-2.5 text-xs" data-testid="fleet-row-note">
              <template v-if="r.level === 'pending'">Waiting for its first sync — the agent is not installed or cannot reach this Photon yet. <RouterLink :to="`/infra/${encodeURIComponent(r.host)}/agent`" class="text-brand hover:underline" @click.stop>Install it →</RouterLink></template>
              <template v-else-if="r.level === 'offline'">Reported, then stopped. Queued work waits for it. <RouterLink :to="`/infra/${encodeURIComponent(r.host)}/agent`" class="text-brand hover:underline" @click.stop>Agent →</RouterLink></template>
              <template v-else>Managed, no resource metrics yet — CPU, memory and disk appear once the agent's sampler reports.</template>
            </TableCell>
          </template>
          <template v-else>
            <TableCell class="py-2.5">
              <div v-if="r.verdict" class="flex flex-wrap items-center gap-1.5" data-testid="fleet-verdict">
                <StatusPill :tone="r.verdict.accent" :dot="false">{{ r.verdict.label }}</StatusPill>
                <span v-if="r.verdict.detail" class="text-xs text-muted-foreground">{{ r.verdict.detail }}</span>
              </div>
              <span v-else-if="r.level === 'offline'" class="text-xs text-sev-error">agent offline — last metrics shown</span>
              <span v-else class="text-xs text-muted-foreground">nothing hot</span>
            </TableCell>
            <TableCell v-for="k in (['cpu', 'mem', 'disk', 'gpu'] as const)" :key="k" class="py-2.5" :data-metric="k">
              <div v-if="meters(r)[k]" class="flex items-center gap-2">
                <Meter :value="meters(r)[k]!.value" :tone="tone(meters(r)[k]!)" class="w-14 flex-none" />
                <span class="whitespace-nowrap font-mono text-[11px] tabular-nums text-foreground" :title="meters(r)[k]!.mean != null ? `worst ${formatPct(meters(r)[k]!.value)} · average ${formatPct(meters(r)[k]!.mean)}` : undefined">{{ formatPct(meters(r)[k]!.value) }}<span v-if="meters(r)[k]!.mean != null" class="text-muted-foreground/60"> / {{ formatPct(meters(r)[k]!.mean) }}</span></span>
              </div>
              <span v-else class="text-xs text-muted-foreground/60">—</span>
            </TableCell>
          </template>

          <TableCell class="py-2.5 text-right font-mono text-xs tabular-nums">
            <span v-if="r.agent?.docker" class="inline-flex items-center gap-1 text-foreground" data-testid="fleet-docker" :title="`${r.agent.docker.containers_running} of ${r.agent.docker.containers_total} containers running`"><Container class="size-3.5 text-muted-foreground" /> {{ r.agent.docker.containers_running }} / {{ r.agent.docker.containers_total }}</span>
            <span v-else class="text-muted-foreground/60">—</span>
          </TableCell>
          <TableCell class="py-2.5">
            <StatusPill v-if="r.agent" :tone="AGENT_STATUS_META[r.agent.status].tone" :solid="r.agent.status === 'offline'" :title="AGENT_STATUS_META[r.agent.status].hint">{{ r.agent.status === 'pending' ? 'pending' : AGENT_STATUS_META[r.agent.status].label.toLowerCase() }}</StatusPill>
            <Badge v-else variant="outline" class="font-normal text-muted-foreground" title="Reports metrics; not enrolled for Docker management">metrics only</Badge>
          </TableCell>
          <TableCell class="whitespace-nowrap py-2.5 text-right text-xs text-muted-foreground">
            {{ r.metrics ? relative(BigInt(r.metrics.lastSeenNs)) : r.agent?.last_seen_ms ? ageMs(r.agent.last_seen_ms) : 'never' }}
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
    <EmptyState v-else :title="query ? 'No host matches the filter' : 'No host at this level'" description="Try a shorter name or clear the filter." class="border-t border-border py-8" />
  </section>
</template>
