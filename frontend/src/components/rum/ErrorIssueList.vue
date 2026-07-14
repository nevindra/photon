<script setup>
// Grouped RUM error issues (by fingerprint), from `useRumErrors`/`useRumPageDetail`:
// { fingerprint, exception_type, message, count, sessions }, ordered by count desc.
// Modeled on `services/DependencyTable.vue` (bordered card + hand-rolled table + a
// `data-testid` per row for tests).
import { RouterLink } from 'vue-router'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { formatNumber } from '@/lib/core/format'

const props = defineProps({
  issues: { type: Array, default: () => [] },
  // The RUM app / backend service these errors belong to — seeds the per-row Related menu's
  // cross-signal jumps (Logs, Backend service, and Trace when a fingerprint carries a trace_id).
  service: { type: String, default: '' },
})

// Each row's Type cell links to the issue-detail view for this fingerprint.
const detailTo = (fingerprint) => `/rum/${encodeURIComponent(props.service)}/errors/${encodeURIComponent(fingerprint)}`
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-xl border border-border bg-card">
    <header class="px-4 pb-2 pt-3.5">
      <h3 class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Errors</h3>
    </header>

    <Table v-if="issues.length" container-class="overflow-visible" class="border-collapse font-mono text-xs">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead>Type</TableHead>
          <TableHead>Message</TableHead>
          <TableHead class="text-right">Count</TableHead>
          <TableHead class="text-right">Sessions</TableHead>
          <TableHead class="text-right"><span class="sr-only">Related</span></TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="issue in issues"
          :key="issue.fingerprint"
          data-testid="rum-issue-row"
          :data-fingerprint="issue.fingerprint"
          class="border-border/60"
        >
          <TableCell class="py-1.5 text-sev-error">
            <RouterLink :to="detailTo(issue.fingerprint)" class="text-brand hover:underline">{{ issue.exception_type }}</RouterLink>
          </TableCell>
          <TableCell class="max-w-md truncate py-1.5 font-sans text-foreground" :title="issue.message">{{ issue.message }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatNumber(issue.count ?? 0) }}</TableCell>
          <TableCell class="py-1.5 text-right tabular-nums text-muted-foreground">{{ formatNumber(issue.sessions ?? 0) }}</TableCell>
          <TableCell class="py-1.5 text-right">
            <RelatedMenu :entity="{ kind: 'rumError', fields: { service, traceId: issue.trace_id } }" />
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <EmptyState
      v-else
      title="No errors in range"
      description="Widen the time range to see JS errors."
      class="h-auto py-8"
    />
  </div>
</template>
