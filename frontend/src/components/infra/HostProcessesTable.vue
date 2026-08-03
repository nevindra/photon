<script setup lang="ts">
// The per-host Processes table: one row per supervised process (`service.name`) on the host, with
// its latest window resource usage. Built on the shared `ui/table` primitives like ServicesTable/
// RedTable. Every header is click-to-sort; the default is CPU descending (heaviest process on top),
// and null numeric values always sort last regardless of direction (a missing metric is the least
// interesting). Rows come from `useInfraHostProcesses` in the parent view.
import { computed, ref } from 'vue'
import { ArrowUp, ArrowDown } from 'lucide-vue-next'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { formatBytes, formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'
import type { InfraProcess } from '@/lib/core/api'

const props = defineProps<{
  processes: InfraProcess[]
  loading?: boolean
}>()

type ProcSortKey = 'process' | 'cpuPct' | 'rssBytes' | 'fds' | 'threads' | 'restarts'
type ColAlign = 'left' | 'right'

const COLUMNS: { key: ProcSortKey; label: string; align: ColAlign }[] = [
  { key: 'process', label: 'Process', align: 'left' },
  { key: 'cpuPct', label: 'CPU %', align: 'right' },
  { key: 'rssBytes', label: 'RSS', align: 'right' },
  { key: 'fds', label: 'FDs', align: 'right' },
  { key: 'threads', label: 'Threads', align: 'right' },
  { key: 'restarts', label: 'Restarts', align: 'right' },
]

const sortKey = ref<ProcSortKey>('cpuPct')
const sortDir = ref<'asc' | 'desc'>('desc')

function onSort(key: ProcSortKey): void {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 'asc' ? 'desc' : 'asc'
  } else {
    sortKey.value = key
    // Text sorts read best ascending; numeric columns default to heaviest-first.
    sortDir.value = key === 'process' ? 'asc' : 'desc'
  }
}

const sortedProcesses = computed<InfraProcess[]>(() => {
  const rows = [...props.processes]
  const key = sortKey.value
  const dir = sortDir.value === 'asc' ? 1 : -1
  rows.sort((a, b) => {
    if (key === 'process') return String(a.process).localeCompare(String(b.process)) * dir
    // Nulls sort to the bottom regardless of direction (missing metric = least interesting).
    const av = a[key]
    const bv = b[key]
    if (av == null && bv == null) return 0
    if (av == null) return 1
    if (bv == null) return -1
    return (Number(av) - Number(bv)) * dir
  })
  return rows
})

function fmtPct(v: number | null): string {
  return v == null ? '—' : v.toFixed(1) + '%'
}
function fmtCount(v: number | null): string {
  return v == null ? '—' : formatNumber(Math.round(v))
}
</script>

<template>
  <section class="flex flex-col gap-2" data-testid="host-processes">
    <div class="flex items-center gap-2.5 text-xs text-muted-foreground">
      <h2 class="text-sm font-medium text-foreground">Processes</h2>
      <span class="font-mono tabular-nums">{{ formatNumber(processes.length) }}</span>
    </div>

    <div class="flex flex-col rounded-lg border border-border bg-card shadow-1">
      <Table container-class="overflow-x-auto" class="text-sm">
        <TableHeader>
          <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
            <TableHead
              v-for="col in COLUMNS"
              :key="col.key"
              :data-testid="'sort-' + col.key"
              :class="cn('cursor-pointer select-none', col.align === 'right' && 'text-right')"
              @click="onSort(col.key)"
            >
              <span class="inline-flex items-center gap-1" :class="col.align === 'right' && 'flex-row-reverse'">
                {{ col.label }}
                <component :is="sortDir === 'desc' ? ArrowDown : ArrowUp" v-if="sortKey === col.key" class="size-3" />
              </span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody v-if="processes.length">
          <TableRow
            v-for="p in sortedProcesses"
            :key="p.process"
            data-testid="process-row"
            :data-process="p.process"
            class="border-border/60"
          >
            <TableCell class="py-1.5 font-mono text-foreground">{{ p.process }}</TableCell>
            <TableCell class="py-1.5 text-right tabular-nums">{{ fmtPct(p.cpuPct) }}</TableCell>
            <TableCell class="py-1.5 text-right tabular-nums">{{ formatBytes(p.rssBytes) }}</TableCell>
            <TableCell class="py-1.5 text-right tabular-nums">{{ fmtCount(p.fds) }}</TableCell>
            <TableCell class="py-1.5 text-right tabular-nums">{{ fmtCount(p.threads) }}</TableCell>
            <TableCell class="py-1.5 text-right tabular-nums">{{ fmtCount(p.restarts) }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>

      <EmptyState
        v-if="!loading && !processes.length"
        title="No processes reporting on this host"
        description="No supervised process is emitting `process.*` metrics in this time range."
        class="h-auto flex-1"
      />
    </div>
  </section>
</template>
