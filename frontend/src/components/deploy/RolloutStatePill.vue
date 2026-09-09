<script setup lang="ts">
import { Loader2 } from 'lucide-vue-next'
import { StatusPill } from '@/components/ui/status-pill'
import { ROLLOUT_STATE_META } from '@/lib/deploy/rollouts'
import type { RolloutState } from '@/lib/core/api'

defineProps<{ state: RolloutState; solid?: boolean }>()
</script>

<template>
  <StatusPill :tone="ROLLOUT_STATE_META[state].tone" :solid="solid && state === 'paused_threshold'" :dot="state !== 'running'" :title="ROLLOUT_STATE_META[state].hint" data-testid="rollout-state" :data-state="state">
    <Loader2 v-if="state === 'running'" class="animate-spin" />
    {{ ROLLOUT_STATE_META[state].label }}
  </StatusPill>
</template>
