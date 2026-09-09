<script setup lang="ts">
// Two immutable versions of one template, compose against compose. Reuses the deploy preview's
// DiffView, so "what changed between 2.3.7 and 2.4.1" reads the same as "what changes on the host".
import { computed } from 'vue'
import DiffView from './DiffView.vue'
import { Spinner } from '@/components/ui/spinner'
import { useTemplateVersion } from '@/lib/deploy/deployQueries'

const props = defineProps<{ fromId: string; toId: string; fromLabel: string; toLabel: string }>()
const from = useTemplateVersion(() => props.fromId)
const to = useTemplateVersion(() => props.toId)
const ready = computed(() => !!from.data.value && !!to.data.value)
</script>

<template>
  <div v-if="!ready" class="flex justify-center py-4"><Spinner size="sm">Loading both versions…</Spinner></div>
  <DiffView v-else :before="from.data.value!.compose_text" :after="to.data.value!.compose_text" :title="`compose.yaml — ${fromLabel} → ${toLabel}`" />
</template>
