<script setup lang="ts">
// Pull (refresh a tag) or remove (free its bytes) one image. Remove is offered only for images no
// container uses, and still asks once — a 2 GB layer set on a slow customer link is a real loss.
import { computed } from 'vue'
import { Loader2 } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { formatBytes } from '@/lib/core/format'
import type { AgentStatus, ImageInfo } from '@/lib/core/api'

const props = defineProps<{ open: boolean; kind: 'image_pull' | 'image_remove' | null; image: ImageInfo | null; hostStatus: AgentStatus; pending?: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; confirm: [] }>()
const ref = computed(() => props.image?.repo_tags[0] ?? props.image?.id.replace(/^sha256:/, '').slice(0, 12) ?? '')
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent v-if="image && kind" class="max-w-md" data-testid="image-action" :data-kind="kind">
      <DialogHeader>
        <DialogTitle>{{ kind === 'image_pull' ? 'Pull image' : 'Remove image' }}</DialogTitle>
        <p class="break-all font-mono text-sm text-foreground" data-testid="image-action-ref">{{ ref }}</p>
        <DialogDescription>
          <template v-if="kind === 'image_pull'">Pulls the tag from its registry. Running containers keep the layers they started with; recreate them to use the new image.</template>
          <template v-else>Frees {{ formatBytes(image.size_bytes) }} on the host. The image is pulled again on the next deploy that needs it{{ hostStatus !== 'online' ? '' : '' }} — on a slow link that costs minutes.</template>
        </DialogDescription>
      </DialogHeader>
      <p v-if="hostStatus !== 'online'" class="text-xs text-muted-foreground">The host is offline — this waits as pending and runs on the next sync, or expires in 15 minutes.</p>
      <DialogFooter>
        <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <Button :variant="kind === 'image_remove' ? 'destructive' : 'default'" :disabled="pending" data-testid="image-action-submit" @click="emit('confirm')"><Loader2 v-if="pending" class="animate-spin" />{{ hostStatus !== 'online' ? 'Queue ' : '' }}{{ kind === 'image_pull' ? 'Pull' : 'Remove' }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
