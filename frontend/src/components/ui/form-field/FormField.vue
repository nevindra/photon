<script setup lang="ts">
import { Label } from '@/components/ui/label'

defineProps<{
  label?: string
  for?: string
  hint?: string
  error?: string
  optional?: boolean
}>()

defineSlots<{
  default(): any
  hint?(): any
}>()
</script>

<template>
  <div class="space-y-1.5">
    <!-- Label Row -->
    <div v-if="label" class="flex items-center justify-between">
      <Label :for="for">{{ label }}</Label>
      <span v-if="optional" class="text-xs text-muted-foreground">Optional</span>
    </div>

    <!-- Control Slot -->
    <slot />

    <!-- Message Row -->
    <p v-if="error" class="text-xs font-mono text-sev-error">{{ error }}</p>
    <slot v-else-if="hint" name="hint">
      <p class="text-xs text-muted-foreground">{{ hint }}</p>
    </slot>
  </div>
</template>
