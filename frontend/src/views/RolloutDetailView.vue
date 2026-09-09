<script setup lang="ts">
// `/deploy/rollouts/:id` — the stage board with the gate drawn between stages, the verbs a state
// allows (start / promote / pause / resume / retry / abort / roll back) each behind a sentence, and
// the rollout's own audit trail.
import { computed, ref } from 'vue'
import { useRoute, useRouter, RouterLink } from 'vue-router'
import { ArrowLeft, Play, ArrowRight, Pause, RotateCcw, Square, RefreshCw, Loader2 } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import RolloutStatePill from '@/components/deploy/RolloutStatePill.vue'
import RolloutProgressBar from '@/components/deploy/RolloutProgressBar.vue'
import RolloutStageBoard from '@/components/deploy/RolloutStageBoard.vue'
import RolloutActionDialog from '@/components/deploy/RolloutActionDialog.vue'
import AuditTable from '@/components/deploy/AuditTable.vue'
import CanaryVerdictPanel from '@/components/deploy/CanaryVerdictPanel.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { api, type RolloutActionKind } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import { backTo } from '@/lib/core/useBackTo'
import { can } from '@/lib/core/auth'
import { permissionHint } from '@/lib/deploy/roles'
import { useRollout, useRolloutAction, useAudit } from '@/lib/deploy/deployQueries'
import { ROLLOUT_STATE_META, availableRolloutActions, selectorSummary, strategySummary } from '@/lib/deploy/rollouts'
import { ageMs, stampMs } from '@/lib/deploy/audit'

const route = useRoute()
const router = useRouter()
const id = computed(() => String(Array.isArray(route.params.id) ? route.params.id[0] : route.params.id ?? ''))
const q = useRollout(id)
const rollout = computed(() => q.data.value ?? null)
const loading = computed(() => q.isLoading.value)
const blocked = computed(() => (can('deploy') ? null : permissionHint('deploy')))
const actions = computed(() => (rollout.value ? availableRolloutActions(rollout.value) : []))

const action = useRolloutAction()
const dialogOpen = ref(false)
const dialogKind = ref<RolloutActionKind | null>(null)
function ask(kind: RolloutActionKind): void {
  dialogKind.value = kind
  dialogOpen.value = true
}
async function onConfirm(): Promise<void> {
  if (!rollout.value || !dialogKind.value) return
  const res = await action.mutateAsync({ id: rollout.value.id, kind: dialogKind.value })
  if (res.ok) dialogOpen.value = false
}
const ACTION = {
  start: { label: 'Start', icon: Play, variant: 'brand' },
  promote: { label: 'Promote', icon: ArrowRight, variant: 'brand' },
  pause: { label: 'Pause', icon: Pause, variant: 'outline' },
  resume: { label: 'Resume anyway', icon: Play, variant: 'outline' },
  retry: { label: 'Retry failed', icon: RefreshCw, variant: 'brand' },
  abort: { label: 'Abort', icon: Square, variant: 'outline' },
  rollback: { label: 'Roll back', icon: RotateCcw, variant: 'outline' },
} as const

const auditQ = useAudit(computed(() => ({ q: id.value, limit: 100 })))
const history = computed(() => (auditQ.data.value?.entries ?? []).filter((e) => e.target.rollout === id.value))
function back(): void {
  backTo(router, '/deploy/rollouts')
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="`Deploy › Rollouts › ${rollout?.name ?? '…'}`" :time-picker="false">
    <template #lead><Button size="icon" variant="ghost" class="size-7 shrink-0" aria-label="Back to rollouts" @click="back"><ArrowLeft class="size-4" /></Button></template>
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="rollout-detail">
      <DeployUnavailable v-if="isDeployUnavailable(q.error.value)" what="Rollouts" />
      <div v-else-if="loading && !rollout" class="flex flex-1 items-center justify-center py-16"><Spinner size="lg">Loading rollout…</Spinner></div>
      <Card v-else-if="!rollout" class="py-4"><EmptyState title="No such rollout"><RouterLink to="/deploy/rollouts" class="mt-3 text-sm text-brand hover:underline">Back to rollouts →</RouterLink></EmptyState></Card>
      <template v-else>
        <Card class="p-4" data-testid="rollout-header">
          <div class="flex flex-wrap items-start gap-4">
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-2">
                <h1 class="text-lg font-semibold text-foreground">{{ rollout.name }}</h1>
                <RolloutStatePill :state="rollout.state" solid />
                <RouterLink :to="`/deploy/templates/${encodeURIComponent(rollout.template_id)}`"><Badge variant="brand" class="font-mono">{{ rollout.template_name }} {{ rollout.template_version }}</Badge></RouterLink>
              </div>
              <p class="mt-0.5 text-xs text-muted-foreground">{{ ROLLOUT_STATE_META[rollout.state].hint }}</p>
              <dl class="mt-3 grid grid-cols-2 gap-x-6 gap-y-1 text-xs sm:grid-cols-4">
                <dt class="text-muted-foreground">Targets</dt><dd class="text-foreground">{{ selectorSummary(rollout.selector) }} · stack <span class="font-mono">{{ rollout.stack_name }}</span></dd>
                <dt class="text-muted-foreground">Strategy</dt><dd class="text-foreground">{{ strategySummary(rollout.strategy) }}</dd>
                <dt class="text-muted-foreground">From</dt><dd class="font-mono text-foreground">{{ rollout.previous_version ?? 'fresh install' }}</dd>
                <dt class="text-muted-foreground">Created</dt><dd class="text-foreground">{{ stampMs(rollout.created_ms) }} by {{ rollout.created_by }}<span v-if="rollout.finished_ms"> · finished {{ ageMs(rollout.finished_ms) }}</span></dd>
              </dl>
              <div class="mt-3 max-w-xl"><RolloutProgressBar :targets="rollout.targets" /></div>
            </div>
            <div class="flex flex-wrap items-center gap-2" data-testid="rollout-actions">
              <GatedButton v-for="k in actions" :key="k" perm="deploy" size="sm" :variant="ACTION[k].variant" :disabled="!!blocked || action.isPending.value" :title="blocked ?? undefined" :data-action="k" @click="ask(k)">
                <Loader2 v-if="action.isPending.value" class="animate-spin" /><component :is="ACTION[k].icon" v-else class="size-4" /> {{ ACTION[k].label }}{{ k === 'rollback' && rollout.previous_version ? ` to ${rollout.previous_version}` : '' }}
              </GatedButton>
            </div>
          </div>
        </Card>
        <CanaryVerdictPanel v-if="rollout.state === 'paused_gate' || rollout.state === 'running'" :rollout="rollout" />
        <RolloutStageBoard :rollout="rollout" />
        <section data-testid="rollout-history">
          <h3 class="mb-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground">History</h3>
          <AuditTable :entries="history" :loading="auditQ.isLoading.value" />
        </section>
        <RolloutActionDialog :open="dialogOpen" :kind="dialogKind" :rollout="rollout" :pending="action.isPending.value" @update:open="dialogOpen = $event" @confirm="onConfirm" />
      </template>
    </main>
  </AppShell>
</template>
