<script setup lang="ts">
// The rollout as stages: the canary, then the rest, each a panel of target rows (host, Site,
// status, attempts, the error in full, a link to the stack). Skipped targets (requirements not
// met) sit in their own panel so they are visible but not in the way. The current stage is
// outlined; the gate between stages is drawn, because the gate IS the safety feature.
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { Server, ArrowDown, Lock } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import TargetStatusPill from './TargetStatusPill.vue'
import RolloutProgressBar from './RolloutProgressBar.vue'
import { ageMs } from '@/lib/deploy/audit'
import { cn } from '@/lib/core/utils'
import type { Rollout } from '@/lib/core/api'

const props = defineProps<{ rollout: Rollout }>()
const stages = computed(() => Array.from({ length: props.rollout.stages }, (_, i) => i + 1).map((n) => ({ n, targets: props.rollout.targets.filter((t) => t.stage === n) })))
const skipped = computed(() => props.rollout.targets.filter((t) => t.stage === 0))
function stageState(n: number): 'done' | 'current' | 'todo' {
  return n < props.rollout.stage ? 'done' : n === props.rollout.stage ? 'current' : 'todo'
}
</script>

<template>
  <div class="flex flex-col gap-3" data-testid="stage-board">
    <template v-for="s in stages" :key="s.n">
      <section :class="cn('overflow-hidden rounded-xl border bg-card shadow-1', stageState(s.n) === 'current' && rollout.state !== 'completed' ? 'border-brand/50' : 'border-border')" :data-stage="s.n" :data-stage-state="stageState(s.n)">
        <header class="flex flex-wrap items-center gap-3 border-b border-border px-4 py-2.5">
          <span :class="cn('flex size-6 items-center justify-center rounded-full text-[11px] font-semibold', stageState(s.n) === 'done' || rollout.state === 'completed' ? 'bg-success text-white' : stageState(s.n) === 'current' ? 'bg-brand text-brand-foreground' : 'bg-muted text-muted-foreground')">{{ s.n }}</span>
          <h3 class="text-sm font-medium text-foreground">{{ s.n === 1 && rollout.stages > 1 ? 'Canary' : rollout.stages > 1 ? 'The rest' : 'All targets' }}</h3>
          <span class="text-xs text-muted-foreground">{{ s.targets.length }} {{ s.targets.length === 1 ? 'target' : 'targets' }}</span>
          <div class="ml-auto w-48"><RolloutProgressBar :targets="s.targets" compact /></div>
        </header>
        <table class="w-full text-sm">
          <tbody>
            <tr v-for="t in s.targets" :key="t.host" class="border-t border-border/60 first:border-0" data-testid="target-row" :data-host="t.host" :data-status="t.status">
              <td class="w-56 whitespace-nowrap px-4 py-2"><span class="inline-flex items-center gap-1.5 font-mono text-xs text-foreground"><Server class="size-3 text-muted-foreground" /> {{ t.host }}</span><Badge v-if="t.site" variant="outline" class="ml-1.5">{{ t.site }}</Badge></td>
              <td class="w-32 px-2 py-2"><TargetStatusPill :status="t.status" /></td>
              <td class="px-2 py-2 text-xs">
                <span v-if="t.last_error" class="text-sev-error">{{ t.last_error }}</span>
                <span v-else-if="t.status === 'pending' && t.agent_status !== 'online'" class="text-muted-foreground">host is {{ t.agent_status === 'offline' ? 'offline' : 'waiting for its first sync' }} — applies on its next sync</span>
                <span v-else-if="t.status === 'queued'" class="text-muted-foreground">starts when stage {{ t.stage }} is promoted</span>
                <span v-else-if="t.status === 'healthy'" class="text-muted-foreground">{{ t.previous_version ?? 'new' }} → {{ rollout.template_version }}</span>
                <span v-else class="text-muted-foreground/60">—</span>
              </td>
              <td class="w-20 px-2 py-2 text-right text-xs tabular-nums text-muted-foreground" :title="`${t.attempts} attempts`">{{ t.attempts }}×</td>
              <td class="w-24 px-2 py-2 text-right text-xs tabular-nums text-muted-foreground">{{ ageMs(t.updated_ms) }}</td>
              <td class="w-16 px-4 py-2 text-right text-xs"><RouterLink v-if="t.stack_id" :to="`/deploy/stacks/${encodeURIComponent(t.stack_id)}`" class="text-brand hover:underline">stack</RouterLink></td>
            </tr>
          </tbody>
        </table>
      </section>
      <div v-if="s.n < rollout.stages" class="flex items-center gap-2 pl-4 text-xs text-muted-foreground" data-testid="stage-gate">
        <span :class="cn('flex size-5 items-center justify-center rounded-full border', rollout.state === 'paused_gate' && rollout.stage === s.n ? 'border-sev-warn bg-sev-warn-soft text-sev-warn' : 'border-border')"><Lock v-if="rollout.state === 'paused_gate' && rollout.stage === s.n" class="size-3" /><ArrowDown v-else class="size-3" /></span>
        <span v-if="rollout.state === 'paused_gate' && rollout.stage === s.n" class="font-medium text-sev-warn">Gate — the canary is done. Promote to continue, or roll back.</span>
        <span v-else-if="rollout.stage > s.n">Gate passed{{ rollout.strategy.auto_promote ? ' (auto)' : '' }}.</span>
        <span v-else>Gate — an operator promotes after the canary is healthy{{ rollout.strategy.auto_promote ? ' (or it auto-promotes)' : '' }}.</span>
      </div>
    </template>
    <section v-if="skipped.length" class="overflow-hidden rounded-xl border border-dashed border-border bg-card" data-testid="skipped-targets">
      <header class="flex items-center gap-2 border-b border-border px-4 py-2 text-xs"><span class="font-medium text-foreground">Skipped</span><span class="text-muted-foreground">requirements not met — not counted as failures</span></header>
      <ul class="divide-y divide-border/60 text-xs">
        <li v-for="t in skipped" :key="t.host" class="flex items-center gap-3 px-4 py-2"><span class="font-mono text-foreground">{{ t.host }}</span><span class="text-muted-foreground">{{ t.last_error }}</span></li>
      </ul>
    </section>
  </div>
</template>
