<script setup lang="ts">
// One template in the picker: plain-English condition + severity, with Apply / Customize.
import { computed } from 'vue'
import { Button } from '@/components/ui/button'
import { StatusPill } from '@/components/ui/status-pill'
import { summarizeCondition, fmtSecs, type AlertTemplate } from '@/lib/alertTemplates'

const props = defineProps<{ template: AlertTemplate; disabled: boolean }>()
defineEmits<{ apply: []; customize: [] }>()

const summary = computed(() => summarizeCondition(props.template.build('…')))
const sevTone = computed(() => (props.template.severity === 'critical' ? 'error' : 'warning'))
</script>

<template>
  <div class="flex items-center justify-between gap-4 rounded-lg border border-border bg-card px-4 py-3">
    <div class="min-w-0">
      <div class="flex items-center gap-2">
        <span class="text-sm font-medium text-foreground">{{ template.name }}</span>
        <StatusPill :tone="sevTone">{{ template.severity }}</StatusPill>
      </div>
      <p class="mt-0.5 truncate font-mono text-xs text-muted-foreground">
        {{ summary }} · for {{ fmtSecs(template.for_secs) }}
      </p>
    </div>
    <div class="flex shrink-0 items-center gap-2">
      <Button size="sm" variant="ghost" :disabled="disabled" @click="$emit('customize')">Customize</Button>
      <Button size="sm" :disabled="disabled" @click="$emit('apply')">Apply</Button>
    </div>
  </div>
</template>
