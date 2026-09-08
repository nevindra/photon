<script setup lang="ts">
// The audit log table: one row per recorded mutation, newest first, with a click-to-expand detail
// line (what the agent said, the payload digest, a link to the host). Rows stay flat and instant
// per the density rule; the expansion is a second row, not a drawer, because an audit reader
// scans dozens of these and a drawer per row is a click tax.
import { ref } from 'vue'
import { RouterLink } from 'vue-router'
import { ChevronRight, Server } from 'lucide-vue-next'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { StatusPill } from '@/components/ui/status-pill'
import { Badge } from '@/components/ui/badge'
import { EmptyState } from '@/components/ui/empty-state'
import { cn } from '@/lib/core/utils'
import { ROLE_META } from '@/lib/deploy/roles'
import { RESULT_META, actionLabel, targetSummary, stampMs, ageMs } from '@/lib/deploy/audit'
import type { AuditEntry } from '@/lib/core/api'

defineProps<{
  entries: AuditEntry[]
  loading?: boolean
}>()

const openId = ref<string | null>(null)
function toggle(id: string): void {
  openId.value = openId.value === id ? null : id
}
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="audit-table">
    <Table container-class="overflow-x-auto" class="text-sm">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead class="w-8" />
          <TableHead class="w-36">When</TableHead>
          <TableHead class="w-40">Who</TableHead>
          <TableHead>What</TableHead>
          <TableHead>Where</TableHead>
          <TableHead class="w-28 text-right">Result</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody v-if="entries.length">
        <template v-for="e in entries" :key="e.id">
          <TableRow
            data-testid="audit-row"
            :data-audit-id="e.id"
            :data-result="e.result"
            class="cursor-pointer border-border/60"
            :class="openId === e.id && 'bg-muted/40'"
            :aria-expanded="openId === e.id"
            @click="toggle(e.id)"
          >
            <TableCell class="py-2 pr-0 text-muted-foreground">
              <ChevronRight :class="cn('size-3.5 transition-transform', openId === e.id && 'rotate-90')" />
            </TableCell>
            <TableCell class="py-2 whitespace-nowrap">
              <span class="tabular-nums text-foreground" :title="new Date(e.ts_ms).toISOString()">{{ ageMs(e.ts_ms) }}</span>
              <span class="ml-1.5 text-xs text-muted-foreground">{{ stampMs(e.ts_ms) }}</span>
            </TableCell>
            <TableCell class="py-2">
              <span class="font-medium text-foreground">{{ e.actor }}</span>
              <Badge variant="outline" class="ml-1.5 align-middle">{{ ROLE_META[e.role]?.label ?? e.role }}</Badge>
            </TableCell>
            <TableCell class="py-2">
              <span class="text-foreground">{{ actionLabel(e.action) }}</span>
              <span v-if="targetSummary(e).primary" class="ml-1.5 font-mono text-xs text-muted-foreground">{{ targetSummary(e).primary }}</span>
            </TableCell>
            <TableCell class="py-2">
              <span v-if="targetSummary(e).host" class="inline-flex items-center gap-1 font-mono text-xs text-muted-foreground">
                <Server class="size-3" /> {{ targetSummary(e).host }}
              </span>
              <span v-else class="text-xs text-muted-foreground/60">—</span>
            </TableCell>
            <TableCell class="py-2 text-right">
              <StatusPill :tone="RESULT_META[e.result].tone" :title="RESULT_META[e.result].hint">{{ RESULT_META[e.result].label }}</StatusPill>
            </TableCell>
          </TableRow>
          <TableRow v-if="openId === e.id" data-testid="audit-detail" class="bg-muted/30 hover:bg-muted/30">
            <TableCell />
            <TableCell colspan="5" class="py-2.5">
              <div class="flex flex-wrap items-start gap-x-6 gap-y-2 text-xs">
                <div class="min-w-0 flex-1">
                  <p class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Detail</p>
                  <p class="mt-0.5 text-foreground">{{ e.detail ?? 'No detail recorded.' }}</p>
                </div>
                <div v-if="e.payload_digest">
                  <p class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Payload digest</p>
                  <p class="mt-0.5 font-mono text-muted-foreground" :title="'The rendered payload is stored, never logged — secrets stay out of the audit trail.'">{{ e.payload_digest }}</p>
                </div>
                <div>
                  <p class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Action</p>
                  <p class="mt-0.5 font-mono text-muted-foreground">{{ e.action }}</p>
                </div>
                <div v-if="e.target.host" class="self-end">
                  <RouterLink :to="`/infra/${encodeURIComponent(e.target.host)}`" class="text-brand hover:underline" @click.stop>Open {{ e.target.host }} →</RouterLink>
                </div>
              </div>
            </TableCell>
          </TableRow>
        </template>
      </TableBody>
    </Table>
    <EmptyState
      v-if="!loading && !entries.length"
      title="Nothing recorded for these filters"
      description="Every change Photon makes to a host lands here — container actions, stack applies, rollouts, enrolments, role changes."
      class="h-auto py-12"
    />
  </div>
</template>
