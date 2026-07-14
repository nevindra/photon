<script setup>
import { computed } from 'vue'
import { cn } from '@/lib/core/utils'

// Apdex score badge (Task 11) — bands the [0,1] Apdex score into a traffic-light tier: good
// (>=0.94), warn (0.85..0.94), bad (<0.85). Tone classes mirror the success/warning/error palette
// used by StatusDot/StatusPill and RedTable.vue's healthTone() so Apdex reads consistently with
// the rest of the app's health signals. `data-band` is exposed for tests/tooling to assert on the
// banding without depending on Tailwind class strings.
const props = defineProps({
  value: { type: Number, default: null },
})

const TONE = {
  good: { text: 'text-success', soft: 'bg-success-soft' },
  warn: { text: 'text-sev-warn', soft: 'bg-sev-warn-soft' },
  bad: { text: 'text-sev-error', soft: 'bg-sev-error-soft' },
}

const band = computed(() => {
  if (props.value == null) return null
  if (props.value >= 0.94) return 'good'
  if (props.value >= 0.85) return 'warn'
  return 'bad'
})

const tone = computed(() => TONE[band.value] ?? { text: 'text-muted-foreground', soft: 'bg-muted' })

const label = computed(() => (props.value == null ? '—' : props.value.toFixed(2)))
</script>

<template>
  <span
    :data-band="band"
    :class="
      cn(
        'inline-flex items-center rounded-full px-2 py-0.5 font-mono text-xs tabular-nums',
        tone.text,
        tone.soft,
      )
    "
  >
    {{ label }}
  </span>
</template>
