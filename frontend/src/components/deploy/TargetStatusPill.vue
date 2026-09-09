<script setup lang="ts">
import { Loader2 } from 'lucide-vue-next'
import { StatusPill } from '@/components/ui/status-pill'
import { TARGET_STATUS_META } from '@/lib/deploy/rollouts'
import type { TargetStatus } from '@/lib/core/api'

defineProps<{ status: TargetStatus }>()
</script>

<template>
  <StatusPill :tone="TARGET_STATUS_META[status].tone" :dot="status !== 'applying' && status !== 'prepulling'" :title="TARGET_STATUS_META[status].hint" data-testid="target-status" :data-status="status">
    <Loader2 v-if="status === 'applying' || status === 'prepulling'" class="animate-spin" />
    {{ TARGET_STATUS_META[status].label }}
  </StatusPill>
</template>
