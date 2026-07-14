<script setup>
import { computed } from 'vue'
import { STATUS_META } from '@/lib/services/serviceHealth'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  status: { type: String, required: true }, // 'critical' | 'degraded' | 'healthy' | 'idle'
  showLabel: { type: Boolean, default: true },
})
const meta = computed(() => STATUS_META[props.status] ?? STATUS_META.idle)
</script>

<template>
  <span
    :data-status="status"
    :class="cn('inline-flex items-center gap-1.5 rounded-full text-[11px] font-medium', meta.text, showLabel && cn('px-2 py-0.5', meta.soft))"
  >
    <span :class="cn('size-2 shrink-0 rounded-full', meta.dot)" />
    <span v-if="showLabel">{{ meta.label }}</span>
  </span>
</template>
