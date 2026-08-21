<script setup lang="ts">
// The Overview's SLO row: one card per objective.
//
// One request per objective rather than one batched endpoint, deliberately. A budget is five
// rollup reads server-side and a service has two or three objectives, not fifty; batching would
// buy a little latency and cost the ability to paint the first card while the second is still
// loading — which matters more when one of them is a 90-day window.
//
// Nothing here appears on the services INDEX. An error budget is a per-service promise measured
// over its own window; putting it in a column beside per-window RED numbers would sit two
// different time spans in one row and invite the comparison.
import { computed } from 'vue'
import SloBudgetCard from './SloBudgetCard.vue'
import { Spinner } from '@/components/ui/spinner'
import { useSlos } from '@/lib/services/servicesQueries'

const props = defineProps<{ service: string }>()

const slosQuery = useSlos(() => props.service)
const slos = computed(() => slosQuery.data.value?.slos ?? [])
const loading = computed(() => slosQuery.isFetching.value)
</script>

<template>
  <!-- A service with no objectives gets no section at all. An empty "Objectives" heading with a
       call to action would be a permanent nag on every service nobody has written an SLO for. -->
  <section v-if="loading || slos.length" data-testid="slo-panel" class="flex flex-col gap-2">
    <div class="flex items-center gap-2.5 text-xs text-muted-foreground">
      <span class="font-medium uppercase tracking-wider">Objectives</span>
      <Spinner v-if="loading" size="sm">loading…</Spinner>
    </div>
    <div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
      <SloBudgetCard v-for="slo in slos" :key="slo.id" :service="service" :slo-id="slo.id" />
    </div>
  </section>
</template>
