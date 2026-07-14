<script setup lang="ts">
// Host list table for the Infrastructure page (`/infra`): one row per reporting host with a quick
// CPU/Memory utilization glance (Meter + percentage) and a GPU presence flag. Clicking a row opens
// the host detail view (InfraHostDetailView, `/infra/:host`) — mirrors ServicesTable's row-click
// pattern (role=button + Enter/Space keyboard support). The empty state is owned by the parent
// view (InfraHostsView), which only renders this table once `hosts.length > 0`.
import type { InfraHost } from '@/lib/core/api'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { Meter } from '@/components/ui/meter'

defineProps<{ hosts: InfraHost[]; loading: boolean }>()
const emit = defineEmits<{ select: [host: string] }>()

function pct(v: number | null): string {
  return v == null ? '—' : `${Math.round(v * 100)}%`
}
function open(host: string): void {
  emit('select', host)
}
</script>

<template>
  <div class="flex flex-col rounded-lg border border-border bg-card shadow-1">
    <Table container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader class="sticky top-0 z-10 rounded-t-lg bg-card">
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead>Host</TableHead>
          <TableHead>CPU</TableHead>
          <TableHead>Memory</TableHead>
          <TableHead class="text-right">GPU</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="h in hosts"
          :key="h.host"
          data-testid="infra-host-row"
          :data-host="h.host"
          role="button"
          tabindex="0"
          class="cursor-pointer border-border/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
          @click="open(h.host)"
          @keydown.enter.prevent="open(h.host)"
          @keydown.space.prevent="open(h.host)"
        >
          <TableCell class="py-1.5 font-medium text-foreground">{{ h.host }}</TableCell>
          <TableCell class="py-1.5">
            <div class="flex items-center gap-2">
              <Meter :value="h.cpuUtil ?? 0" class="w-14 shrink-0" />
              <span class="tabular-nums text-muted-foreground">{{ pct(h.cpuUtil) }}</span>
            </div>
          </TableCell>
          <TableCell class="py-1.5">
            <div class="flex items-center gap-2">
              <Meter :value="h.memUtil ?? 0" class="w-14 shrink-0" />
              <span class="tabular-nums text-muted-foreground">{{ pct(h.memUtil) }}</span>
            </div>
          </TableCell>
          <TableCell class="py-1.5 text-right text-muted-foreground">{{ h.hasGpu ? 'Yes' : '—' }}</TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
</template>
