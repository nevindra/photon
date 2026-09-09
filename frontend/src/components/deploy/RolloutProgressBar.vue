<script setup lang="ts">
// One bar, one segment per target, coloured by status: how far a rollout is at a glance. Pending
// stays neutral — on-prem, waiting is the normal state, and a bar that painted it red would make
// every offline site look like an incident.
import { computed } from 'vue'
import { targetCounts } from '@/lib/deploy/rollouts'
import type { RolloutTarget } from '@/lib/core/api'

const props = defineProps<{ targets: RolloutTarget[]; compact?: boolean }>()
const c = computed(() => targetCounts(props.targets))
const SEG = [
  { key: 'healthy', cls: 'bg-success', label: 'healthy' },
  { key: 'inflight', cls: 'bg-brand', label: 'in flight' },
  { key: 'failed', cls: 'bg-sev-error', label: 'failed' },
  { key: 'drifted', cls: 'bg-sev-warn', label: 'drifted' },
  { key: 'pending', cls: 'bg-muted-foreground/30', label: 'pending' },
  { key: 'queued', cls: 'bg-muted-foreground/15', label: 'queued' },
  { key: 'skipped', cls: 'bg-muted-foreground/15', label: 'skipped' },
] as const
const pct = (n: number) => (c.value.total ? (n / c.value.total) * 100 : 0)
</script>

<template>
  <div class="flex flex-col gap-1" data-testid="rollout-progress">
    <div class="flex h-2 w-full overflow-hidden rounded-full bg-muted" role="img" :aria-label="`${c.healthy} of ${c.total} healthy`">
      <span v-for="s in SEG" :key="s.key" :class="s.cls" :style="{ width: pct(c[s.key]) + '%' }" :title="`${c[s.key]} ${s.label}`" />
    </div>
    <div v-if="!compact" class="flex flex-wrap gap-x-3 text-[11px] text-muted-foreground">
      <span class="tabular-nums text-foreground">{{ c.healthy }} / {{ c.total }} healthy</span>
      <span v-if="c.inflight" class="text-brand">{{ c.inflight }} in flight</span>
      <span v-if="c.failed" class="text-sev-error">{{ c.failed }} failed</span>
      <span v-if="c.drifted" class="text-sev-warn">{{ c.drifted }} drifted</span>
      <span v-if="c.pending">{{ c.pending }} pending</span>
      <span v-if="c.queued">{{ c.queued }} queued</span>
      <span v-if="c.skipped">{{ c.skipped }} skipped</span>
    </div>
  </div>
</template>
