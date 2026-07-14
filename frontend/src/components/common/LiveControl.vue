<script setup>
import { computed } from 'vue'
import { SelectMenu } from '@/components/ui/select-menu'
import { StatusDot } from '@/components/ui/status-dot'
import { Button } from '@/components/ui/button'
import { RefreshCw } from 'lucide-vue-next'

const props = defineProps({
  mode: { type: String, default: 'manual' },
  status: { type: String, default: 'idle' },
  rate: { type: Number, default: null },
  streamable: { type: Boolean, default: true },
})
const emit = defineEmits(['update:mode', 'refresh'])

const ALL_MODES = [
  { key: 'manual', label: 'Manual' },
  { key: '5s', label: '5s' },
  { key: '30s', label: '30s' },
  { key: 'live', label: 'Live' },
]

const visibleModes = computed(() => ALL_MODES.filter((m) => m.key !== 'live' || props.streamable))

const dotTone = computed(
  () => ({ live: 'success', lagged: 'warning', reconnecting: 'neutral', idle: 'neutral' })[props.status],
)
const isPulsing = computed(() => props.status === 'live')

const compactRate = computed(() => {
  if (props.mode !== 'live' || props.rate == null) return null
  return props.rate >= 1000 ? `${(props.rate / 1000).toFixed(1)}k/s` : `${props.rate}/s`
})
</script>

<template>
  <div class="flex items-center gap-2">
    <SelectMenu
      :model-value="mode"
      :options="visibleModes.map((m) => ({ value: m.key, label: m.label }))"
      aria-label="Refresh mode"
      @update:model-value="(v) => v && emit('update:mode', v)"
    />

    <Button
      variant="ghost"
      size="icon"
      data-testid="live-refresh"
      aria-label="Refresh now"
      @click="emit('refresh')"
    >
      <RefreshCw class="size-4" />
    </Button>

    <span v-if="status !== 'idle'" class="flex items-center gap-1.5 text-xs text-muted-foreground">
      <StatusDot :tone="dotTone" :class="isPulsing ? 'animate-pulse' : ''" />
      <span class="font-mono">{{ status === 'live' ? 'LIVE' : status }}</span>
      <span v-if="compactRate" class="text-muted-foreground/70">· {{ compactRate }}</span>
    </span>
  </div>
</template>
