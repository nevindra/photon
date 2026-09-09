<script setup lang="ts">
// Confirmations for the rollout verbs. Promote says how many hosts the next stage touches; abort
// says what stays; rollback (R6.5) says it is just another apply of the previous version and to
// how many hosts — the whole point of immutable versions is that this needs no separate machinery.
import { computed } from 'vue'
import { Loader2 } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import type { Rollout, RolloutActionKind } from '@/lib/core/api'

const props = defineProps<{ open: boolean; kind: RolloutActionKind | null; rollout: Rollout | null; pending?: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; confirm: [] }>()

const META = computed(() => {
  const r = props.rollout
  if (!r || !props.kind) return null
  const next = r.targets.filter((t) => t.stage === r.stage + 1).length
  const touched = r.targets.filter((t) => t.status === 'healthy' || t.status === 'failed' || t.status === 'drifted').length
  const failed = r.targets.filter((t) => t.stage === r.stage && t.status === 'failed').length
  switch (props.kind) {
    case 'start':
      return { title: `Start ${r.name}?`, body: `Stage 1 applies ${r.template_version} to ${r.targets.filter((t) => t.stage === 1).length} canary target(s)${r.strategy.prepull ? ', pre-pulling images first' : ''}. The rest wait at the gate.`, verb: 'Start', destructive: false }
    case 'promote':
      return { title: `Promote to stage ${r.stage + 1}?`, body: `Applies ${r.template_version} to the remaining ${next} host${next === 1 ? '' : 's'}. Offline hosts take it on their next sync. The stage pauses if ${r.strategy.failure_threshold} fail.`, verb: `Promote ${next} host${next === 1 ? '' : 's'}`, destructive: false }
    case 'pause':
      return { title: 'Pause the rollout?', body: 'Targets already applying finish; nothing new starts until you resume.', verb: 'Pause', destructive: false }
    case 'resume':
      return { title: 'Resume past the threshold?', body: `${failed} target${failed === 1 ? '' : 's'} failed in this stage. Resuming continues with the remaining targets anyway — the failed ones stay failed until retried.`, verb: 'Resume anyway', destructive: true }
    case 'retry':
      return { title: 'Retry the failed targets?', body: `${failed} failed target${failed === 1 ? '' : 's'} in stage ${r.stage} go back to pending and apply again on the next sync.`, verb: 'Retry', destructive: false }
    case 'abort':
      return { title: `Abort ${r.name}?`, body: `Stops the rollout. Hosts already on ${r.template_version} keep it; the rest stay on what they run. Roll back afterwards if you want everything on ${r.previous_version ?? 'the previous version'}.`, verb: 'Abort', destructive: true }
    case 'rollback':
      return { title: `Roll back to ${r.previous_version}?`, body: `Re-applies ${r.previous_version} — the stored, immutable version with the previous variables — to the ${touched} host${touched === 1 ? '' : 's'} this rollout touched. It is an ordinary apply, not a special mechanism.`, verb: `Roll back ${touched} host${touched === 1 ? '' : 's'}`, destructive: true }
  }
})
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent v-if="META" class="max-w-md" data-testid="rollout-action-dialog" :data-kind="kind">
      <DialogHeader><DialogTitle>{{ META.title }}</DialogTitle><DialogDescription>{{ META.body }}</DialogDescription></DialogHeader>
      <DialogFooter>
        <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <Button :variant="META.destructive ? 'destructive' : 'default'" :disabled="pending" data-testid="rollout-action-submit" @click="emit('confirm')"><Loader2 v-if="pending" class="animate-spin" />{{ META.verb }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
