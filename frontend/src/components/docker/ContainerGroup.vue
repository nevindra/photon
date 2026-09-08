<script setup lang="ts">
// One compose project (or the standalone bucket) as a panel: a header with the project's worst
// state, then one flat row per container. Rows stay flat and instant; clicking one opens the
// drawer. Within a project the compose SERVICE is the primary name — that is what the compose
// file calls it — and the container's own name sits under it in mono.
import { RouterLink } from 'vue-router'
import { ChevronRight, Layers, Loader2, ExternalLink } from 'lucide-vue-next'
import ContainerActionMenu from './ContainerActionMenu.vue'
import { ACTION_META } from '@/lib/deploy/actions'
import { StatusDot } from '@/components/ui/status-dot'
import { StatusPill } from '@/components/ui/status-pill'
import { Badge } from '@/components/ui/badge'
import { Meter } from '@/components/ui/meter'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { containerTone, HEALTH_META, STATE_META, formatPort, portUrl, uniquePorts, shortImage, type ProjectGroup } from '@/lib/deploy/containers'
import { formatBytes } from '@/lib/core/format'
import type { ActionKind, Container } from '@/lib/core/api'

defineProps<{
  group: ProjectGroup
  /** The host's name — where a published port opens (`portUrl`). */
  host: string
  /** Inside a parent card: no border or shadow of its own, a hairline above instead. */
  embedded?: boolean
  selectedId?: string | null
  /** Why lifecycle actions are unavailable on this host (role / capability), or null. */
  blocked?: string | null
  /** Container id → the action queued or running for it. */
  busy?: Record<string, ActionKind>
  /** The inventory is the agent's last report, not live: numbers dim and the header says when. */
  stale?: boolean
  asOf?: string
}>()
const emit = defineEmits<{ open: [c: Container]; act: [c: Container, kind: ActionKind] }>()

function memRatio(c: Container): number | null {
  return c.mem_bytes != null && c.mem_limit_bytes ? c.mem_bytes / c.mem_limit_bytes : null
}
</script>

<template>
  <section :class="embedded ? 'border-t border-border' : 'overflow-hidden rounded-xl border border-border bg-card shadow-1'" data-testid="container-group" :data-project="group.project ?? ''">
    <header class="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2.5" :class="embedded && 'bg-muted/40'">
      <StatusDot :tone="group.tone" size="lg" />
      <h3 class="text-sm font-medium text-foreground">
        <template v-if="group.project"><Layers class="mr-1 inline size-3.5 align-[-2px] text-muted-foreground" />{{ group.project }}</template>
        <template v-else>Standalone containers</template>
      </h3>
      <RouterLink v-if="group.managedStackId" :to="`/deploy/stacks/${encodeURIComponent(group.managedStackId)}`" data-testid="managed-chip"><Badge variant="brand" class="hover:bg-brand/15">Managed by Photon →</Badge></RouterLink>
      <span class="ml-auto text-xs tabular-nums text-muted-foreground">{{ group.running }} / {{ group.total }} running<template v-if="stale && asOf"> · as of {{ asOf }}</template></span>
    </header>
    <Table container-class="overflow-x-auto" class="text-sm">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead class="w-6" />
          <TableHead>{{ group.project ? 'Service' : 'Container' }}</TableHead>
          <TableHead>Image</TableHead>
          <TableHead>Status</TableHead>
          <TableHead class="hidden w-20 text-right xl:table-cell">CPU</TableHead>
          <TableHead class="hidden w-52 xl:table-cell">Memory</TableHead>
          <TableHead class="w-20 text-right">Restarts</TableHead>
          <TableHead>Ports</TableHead>
          <TableHead class="w-36 text-right">Actions</TableHead>
          <TableHead class="w-6" />
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="c in group.containers"
          :key="c.id"
          data-testid="container-row"
          :data-container="c.name"
          :data-state="c.state"
          class="cursor-pointer border-border/60"
          :class="[selectedId === c.id && 'bg-brand-soft/60', stale && 'text-muted-foreground']"
          @click="emit('open', c)"
        >
          <TableCell class="py-2 pr-0"><StatusDot :tone="containerTone(c)" size="lg" :title="STATE_META[c.state].label" /></TableCell>
          <TableCell class="max-w-[14rem] py-2">
            <!-- The name is the row's one real control (keyboard + screen reader); the row click is a
                 pointer convenience on top. No role="button" on a <tr> wrapping other buttons. -->
            <button type="button" class="block max-w-full truncate text-left font-medium text-foreground hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring" :title="c.compose_service ?? c.name" @click.stop="emit('open', c)">{{ c.compose_service ?? c.name }}</button>
            <div v-if="c.compose_service" class="truncate font-mono text-[11px] text-muted-foreground" :title="c.name">{{ c.name }}</div>
          </TableCell>
          <TableCell class="max-w-[12rem] truncate py-2 font-mono text-xs text-muted-foreground" :title="c.image">{{ shortImage(c.image) }}</TableCell>
          <TableCell class="py-2">
            <div class="flex flex-wrap items-center gap-1.5">
              <StatusPill v-if="busy?.[c.id]" tone="info" :dot="false" data-testid="container-busy"><Loader2 class="animate-spin" /> {{ ACTION_META[busy[c.id]].progressive }}</StatusPill>
              <span v-else :class="containerTone(c) === 'error' ? 'text-sev-error' : containerTone(c) === 'warning' ? 'text-sev-warn' : stale ? 'text-muted-foreground' : 'text-foreground'" class="whitespace-nowrap text-xs">{{ c.status_text }}</span>
              <StatusPill v-if="HEALTH_META[c.health] && c.health !== 'healthy'" :tone="HEALTH_META[c.health]!.tone">{{ HEALTH_META[c.health]!.label }}</StatusPill>
            </div>
          </TableCell>
          <TableCell class="hidden py-2 text-right tabular-nums xl:table-cell" :class="c.cpu_pct != null && c.cpu_pct >= 80 ? 'text-sev-warn' : stale ? 'text-muted-foreground' : 'text-foreground'">{{ c.cpu_pct == null ? '—' : c.cpu_pct.toFixed(1) + '%' }}</TableCell>
          <TableCell class="hidden py-2 xl:table-cell">
            <div v-if="c.mem_bytes != null" class="flex items-center gap-2">
              <Meter v-if="memRatio(c) != null" :value="memRatio(c)!" :tone="memRatio(c)! >= 0.9 ? 'error' : memRatio(c)! >= 0.75 ? 'warning' : 'info'" class="w-16" />
              <span class="whitespace-nowrap tabular-nums text-xs" :class="stale ? 'text-muted-foreground' : 'text-foreground'">{{ formatBytes(c.mem_bytes) }}<span v-if="c.mem_limit_bytes" class="text-muted-foreground"> / {{ formatBytes(c.mem_limit_bytes) }}</span></span>
            </div>
            <span v-else class="text-xs text-muted-foreground">—</span>
          </TableCell>
          <TableCell class="py-2 text-right tabular-nums" :class="c.restart_count >= 5 ? 'text-sev-error' : c.restart_count > 0 ? 'text-sev-warn' : 'text-muted-foreground'">{{ c.restart_count }}</TableCell>
          <TableCell class="py-2">
            <!-- A published TCP port is a link: it opens the service in a new tab. The row's
                 click must not follow it into the drawer. -->
            <div class="flex flex-wrap gap-1">
              <template v-for="p in uniquePorts(c.ports)" :key="formatPort(p)">
                <a v-if="portUrl(p, host)" :href="portUrl(p, host)!" target="_blank" rel="noopener noreferrer" class="group/port inline-flex" :title="`Open ${portUrl(p, host)} in a new tab`" data-testid="port-link" @click.stop>
                  <Badge variant="outline" class="gap-1 font-mono transition-colors group-hover/port:border-brand group-hover/port:bg-brand-soft group-hover/port:text-brand-strong">{{ formatPort(p) }}<ExternalLink class="size-2.5 opacity-50 group-hover/port:opacity-100" /></Badge>
                </a>
                <Badge v-else variant="outline" class="font-mono">{{ formatPort(p) }}</Badge>
              </template>
              <span v-if="!c.ports.length" class="text-xs text-muted-foreground/60">—</span>
            </div>
          </TableCell>
          <TableCell class="py-1.5 text-right">
            <ContainerActionMenu :container="c" :blocked="blocked ?? null" :busy="busy?.[c.id] ?? null" class="justify-end" @act="emit('act', c, $event)" />
          </TableCell>
          <TableCell class="py-2 text-muted-foreground"><ChevronRight class="size-3.5" /></TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </section>
</template>
