<script setup lang="ts">
import { Loader2 } from 'lucide-vue-next'
import { StatusPill } from '@/components/ui/status-pill'
import { STACK_STATUS_META } from '@/lib/deploy/stacks'
import type { StackStatus } from '@/lib/core/api'

defineProps<{ status: StackStatus; solid?: boolean }>()
</script>

<template>
  <StatusPill :tone="STACK_STATUS_META[status].tone" :solid="solid && status === 'failed'" :dot="status !== 'applying' && status !== 'destroying'" :title="STACK_STATUS_META[status].hint" data-testid="stack-status" :data-status="status">
    <Loader2 v-if="status === 'applying' || status === 'destroying'" class="animate-spin" />
    {{ STACK_STATUS_META[status].label }}
  </StatusPill>
</template>
