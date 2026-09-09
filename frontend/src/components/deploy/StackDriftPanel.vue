<script setup lang="ts">
// Drift: what runs on the host differs from what Photon applied. One row inside the header card,
// not a banner of its own: the change itself — desired → observed, since when — with the two
// deliberate exits beside it. Reconcile reverts, Accept renders what runs as the new desired
// state; each has its own confirmation. Never auto-reverted: the change was probably somebody
// keeping production up. Several differences stack as rows under the one summary line.
import { GitCompareArrows } from 'lucide-vue-next'
import GatedButton from './GatedButton.vue'
import { driftLabel } from '@/lib/deploy/stacks'
import { ageMs } from '@/lib/deploy/audit'
import type { DriftItem } from '@/lib/core/api'

defineProps<{ drift: DriftItem[]; blocked?: string | null }>()
defineEmits<{ reconcile: []; accept: [] }>()
</script>

<template>
  <div v-if="drift.length" class="border-t border-sev-warn/30 bg-sev-warn-soft" data-testid="stack-drift">
    <div class="flex flex-wrap items-center gap-3 px-4 py-2.5">
      <span class="flex size-5 shrink-0 items-center justify-center rounded-full bg-sev-warn text-white"><GitCompareArrows class="size-3" /></span>
      <p class="min-w-0 flex-1 text-xs text-sev-warn">
        <span class="font-semibold">{{ drift.length }} {{ drift.length === 1 ? 'difference' : 'differences' }} between what runs and what was applied.</span>
        Somebody changed this on the host. Revert it, or accept it as desired — doing neither keeps their change and the record stays honest about it.
      </p>
      <div class="flex items-center gap-2">
        <GatedButton perm="deploy" size="sm" variant="outline" class="h-7 bg-card" :disabled="!!blocked" :title="blocked ?? undefined" data-testid="accept-drift" @click="$emit('accept')">Accept as desired…</GatedButton>
        <GatedButton perm="deploy" size="sm" variant="outline" class="h-7 bg-card" :disabled="!!blocked" :title="blocked ?? undefined" data-testid="reconcile" @click="$emit('reconcile')">Reconcile…</GatedButton>
      </div>
    </div>
    <ul class="divide-y divide-sev-warn/15 border-t border-sev-warn/15">
      <li v-for="(d, i) in drift" :key="i" class="flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-1.5 pl-12 text-xs" :data-drift="d.kind">
        <span class="font-medium text-foreground">{{ driftLabel(d) }}</span>
        <span v-if="d.service" class="font-mono text-muted-foreground">{{ d.service }}<template v-if="d.field">.{{ d.field }}</template></span>
        <span class="font-mono"><span class="text-foreground">{{ d.desired ?? '—' }}</span> <span class="text-muted-foreground">→</span> <span class="text-sev-warn">{{ d.observed ?? '—' }}</span></span>
        <span class="ml-auto tabular-nums text-muted-foreground">since {{ ageMs(d.since_ms) }}</span>
      </li>
    </ul>
  </div>
</template>
