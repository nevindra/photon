<script setup>
import { computed } from 'vue'
import { severity, severityClasses } from '@/lib/core/format'

const props = defineProps({
  level: { type: String, required: true },
})

const label = computed(() => severity(props.level).label)
const classes = computed(() => severityClasses(props.level))
</script>

<template>
  <span
    :class="[
      classes.text,
      classes.bgSoft,
      'inline-flex items-center rounded-sm px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wider leading-none',
    ]"
  >
    {{ label }}
  </span>
</template>
