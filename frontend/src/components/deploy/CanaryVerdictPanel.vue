<script setup lang="ts">
// At the gate: what the canary hosts' services did since they were upgraded, judged the same way a
// single deploy is. The argument of the whole proposal made concrete — promote on evidence, not
// on a green apply. Read-only: the buttons stay in the header, this only informs them.
import { computed } from 'vue'
import DeployImpactPanel from './DeployImpactPanel.vue'
import { useStacks } from '@/lib/deploy/deployQueries'
import type { Rollout } from '@/lib/core/api'

const props = defineProps<{ rollout: Rollout }>()
const stacksQ = useStacks()
const canary = computed(() => props.rollout.targets.filter((t) => t.stage === props.rollout.stage && t.status === 'healthy'))
const services = computed(() => {
  const ids = new Set(canary.value.map((t) => t.stack_id).filter((x): x is string => !!x))
  const out = new Set<string>()
  for (const s of stacksQ.data.value?.stacks ?? []) if (ids.has(s.id)) for (const svc of s.apm_services) out.add(svc)
  return Array.from(out)
})
const appliedMs = computed(() => (canary.value.length ? Math.max(...canary.value.map((t) => t.updated_ms)) : null))
</script>

<template>
  <DeployImpactPanel v-if="canary.length" :services="services" :applied-ms="appliedMs" :title="`Canary verdict — stage ${rollout.stage}, ${canary.length} host${canary.length === 1 ? '' : 's'}`" />
</template>
