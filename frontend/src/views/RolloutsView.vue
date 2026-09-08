<script setup lang="ts">
// `/deploy/rollouts` — the ops team's real job: one template version to many hosts, staged. The
// list puts rollouts that need a decision (at the gate, paused on failures) first, then running
// ones, then history.
import { computed, ref } from 'vue'
import { useRouter, RouterLink } from 'vue-router'
import { Plus, ChevronRight } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import NewRolloutDialog from '@/components/deploy/NewRolloutDialog.vue'
import RolloutStatePill from '@/components/deploy/RolloutStatePill.vue'
import RolloutProgressBar from '@/components/deploy/RolloutProgressBar.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { StatTile } from '@/components/ui/stat-tile'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { api } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import { useUrlParam } from '@/lib/core/useUrlState'
import { useRollouts } from '@/lib/deploy/deployQueries'
import { ROLLOUT_STATE_META, rolloutCounts, sortRollouts, selectorSummary } from '@/lib/deploy/rollouts'
import { ageMs } from '@/lib/deploy/audit'
import { formatNumber } from '@/lib/core/format'

const router = useRouter()
const q = useRollouts()
const all = computed(() => q.data.value?.rollouts ?? [])
const loading = computed(() => q.isLoading.value)
const counts = computed(() => rolloutCounts(all.value))
const unavailable = computed(() => isDeployUnavailable(q.error.value))

const filter = ref<string | null>(null)
useUrlParam('show', filter, { valid: (v) => ['decision', 'active', 'done'].includes(v) })
const rows = computed(() => sortRollouts(all.value.filter((r) => {
  const m = ROLLOUT_STATE_META[r.state]
  if (filter.value === 'decision') return m.decision
  if (filter.value === 'active') return m.active || r.state === 'draft'
  if (filter.value === 'done') return !m.active && r.state !== 'draft'
  return true
})))
const dialogOpen = ref(false)
</script>

<template>
  <AppShell :mock="api.mock" crumb="Deploy › Rollouts" :time-picker="false">
    <template #actions>
      <GatedButton perm="deploy" size="sm" data-testid="new-rollout" @click="dialogOpen = true"><Plus class="size-4" /> New rollout</GatedButton>
    </template>
    <section class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 pt-5 pb-6" data-testid="rollouts-view">
      <header>
        <h1 class="text-xl font-semibold text-foreground">Rollouts</h1>
        <p class="text-sm text-muted-foreground">One template version to many hosts, staged: a canary, an explicit gate, then the rest. A site that is offline is pending, not failed.</p>
      </header>
      <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4" data-testid="rollout-kpis">
        <StatTile label="Needs a decision" :value="formatNumber(counts.decision)" :accent="counts.decision ? 'warning' : undefined" sub="at the gate, or paused on failures" />
        <StatTile label="Active" :value="formatNumber(counts.active)" :accent="counts.active ? 'info' : undefined" sub="running or paused" />
        <StatTile label="Completed" :value="formatNumber(counts.completed)" sub="every target settled" />
        <StatTile label="Rolled back" :value="formatNumber(counts.rolledBack)" :accent="counts.rolledBack ? 'warning' : undefined" sub="previous version re-applied" />
      </div>
      <Segmented :model-value="filter ?? ''" class="h-8 w-fit" data-testid="rollout-filter" @update:model-value="filter = $event ? String($event) : null">
        <SegmentedItem value="" class="px-2.5 text-xs">All</SegmentedItem>
        <SegmentedItem value="decision" class="px-2.5 text-xs">Needs a decision</SegmentedItem>
        <SegmentedItem value="active" class="px-2.5 text-xs">Active</SegmentedItem>
        <SegmentedItem value="done" class="px-2.5 text-xs">Done</SegmentedItem>
      </Segmented>

      <DeployUnavailable v-if="unavailable" what="Rollouts" />
      <div v-else-if="loading && !all.length" class="flex flex-1 items-center justify-center py-16"><Spinner size="lg">Loading rollouts…</Spinner></div>
      <div v-else class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="rollouts-table">
        <Table container-class="overflow-x-auto" class="text-sm">
          <TableHeader>
            <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
              <TableHead class="w-36">State</TableHead><TableHead>Rollout</TableHead><TableHead class="w-24">Stage</TableHead><TableHead class="w-64">Targets</TableHead><TableHead class="w-32 text-right">Updated</TableHead><TableHead class="w-6" />
            </TableRow>
          </TableHeader>
          <TableBody v-if="rows.length">
            <TableRow v-for="r in rows" :key="r.id" data-testid="rollout-row" :data-rollout="r.id" :data-state="r.state" class="cursor-pointer border-border/60" @click="router.push(`/deploy/rollouts/${encodeURIComponent(r.id)}`)">
              <TableCell class="py-2"><RolloutStatePill :state="r.state" /></TableCell>
              <TableCell class="py-2"><RouterLink :to="`/deploy/rollouts/${encodeURIComponent(r.id)}`" class="font-medium text-foreground hover:underline" @click.stop>{{ r.name }}</RouterLink><div class="text-[11px] text-muted-foreground">{{ r.template_name }} <span class="font-mono">{{ r.previous_version ? `${r.previous_version} → ` : '' }}{{ r.template_version }}</span> · {{ selectorSummary(r.selector) }} · by {{ r.created_by }}</div></TableCell>
              <TableCell class="py-2 text-xs tabular-nums text-muted-foreground">{{ r.state === 'draft' ? 'not started' : `${r.stage} of ${r.stages}` }}</TableCell>
              <TableCell class="py-2"><RolloutProgressBar :targets="r.targets" /></TableCell>
              <TableCell class="py-2 text-right text-xs tabular-nums text-muted-foreground">{{ ageMs(r.updated_ms) }}</TableCell>
              <TableCell class="py-2 text-muted-foreground"><ChevronRight class="size-3.5" /></TableCell>
            </TableRow>
          </TableBody>
        </Table>
        <EmptyState v-if="!rows.length" title="No rollouts here" description="Start one from a template, or from this page." class="h-auto py-12" />
      </div>
      <NewRolloutDialog :open="dialogOpen" @update:open="dialogOpen = $event" />
    </section>
  </AppShell>
</template>
