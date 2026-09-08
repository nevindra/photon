<script setup lang="ts">
// What a Deploy screen shows on a Photon whose backend has no fleet-management subsystem: the
// truth, not demo data. The "preview" button is the deliberate opt-in to the mock corpus for
// people evaluating the screens; it is labelled as such and marks the shell "demo data".
import { Card } from '@/components/ui/card'
import { EmptyState } from '@/components/ui/empty-state'
import { Button } from '@/components/ui/button'
import { Boxes } from 'lucide-vue-next'
import { setDeployDemo } from '@/lib/deploy/availability'

defineProps<{ what?: string }>()
function preview(): void {
  setDeployDemo(true)
  location.reload()
}
</script>

<template>
  <Card class="py-4" data-testid="deploy-unavailable">
    <EmptyState :title="`${what ?? 'Docker fleet management'} isn't enabled on this server`" description="This Photon has no deploy subsystem yet: no managed agents, stacks, templates or rollouts. The screens are here; the server side is not. Nothing on this page is live.">
      <template #icon><Boxes /></template>
      <Button variant="outline" size="sm" class="mt-3" data-testid="deploy-demo" @click="preview">Preview with demo data</Button>
    </EmptyState>
  </Card>
</template>
