<script setup lang="ts">
// One confirmation for every container action, sized to the blast radius (`ACTION_META.risk`):
// `none` never opens this; `confirm` states what Docker will do and asks once; `typed` (remove)
// wants the container's name. Options ride here too — recreate can pull first, remove can drop
// anonymous volumes behind its own data-loss line. An offline host gets a plain sentence: the
// action waits as pending and expires in 15 minutes; it does not fail.
import { computed, ref, watch } from 'vue'
import { Loader2, WifiOff, TriangleAlert } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { FormField } from '@/components/ui/form-field'
import { ACTION_META } from '@/lib/deploy/actions'
import type { ActionArgs, ActionKind, AgentStatus, Container } from '@/lib/core/api'

const props = defineProps<{
  open: boolean
  kind: ActionKind | null
  container: Container | null
  hostStatus: AgentStatus
  pending?: boolean
}>()
const emit = defineEmits<{ 'update:open': [boolean]; confirm: [args: ActionArgs] }>()

const meta = computed(() => (props.kind ? ACTION_META[props.kind] : null))
const typed = ref('')
const pullFirst = ref(false)
const removeVolumes = ref(false)
watch(
  () => props.open,
  (o) => {
    if (o) {
      typed.value = ''
      pullFirst.value = false
      removeVolumes.value = false
    }
  },
)
const offline = computed(() => props.hostStatus !== 'online')
const canSubmit = computed(() => !!meta.value && (meta.value.risk !== 'typed' || typed.value === props.container?.name))

function submit(): void {
  if (!canSubmit.value || !props.kind) return
  const args: ActionArgs = {}
  if (props.kind === 'recreate') args.pull = pullFirst.value
  if (props.kind === 'remove') args.remove_volumes = removeVolumes.value
  emit('confirm', args)
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent v-if="kind && container && meta" class="max-w-md" data-testid="action-confirm" :data-kind="kind">
      <DialogHeader>
        <DialogTitle>{{ meta.label }}</DialogTitle>
        <p class="break-all font-mono text-sm text-foreground" data-testid="action-confirm-name">{{ container.name }}</p>
        <DialogDescription>{{ meta.explain(container) }}</DialogDescription>
      </DialogHeader>

      <div v-if="offline" class="flex items-start gap-2 rounded-lg border border-border bg-muted px-3 py-2 text-xs" data-testid="action-offline-note">
        <WifiOff class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        <span class="text-foreground">This host is {{ hostStatus === 'offline' ? 'offline' : 'waiting for its first sync' }}. The action waits as <b>pending</b> and runs on the agent's next sync — or expires in 15 minutes so it cannot fire at a bad moment later.</span>
      </div>

      <label v-if="kind === 'recreate'" class="flex cursor-pointer items-start gap-2 text-sm">
        <Checkbox :model-value="pullFirst" class="mt-0.5" data-testid="action-pull-first" @update:model-value="pullFirst = $event === true" />
        <span><span class="text-foreground">Pull the image first</span><span class="block text-xs text-muted-foreground">Fetch {{ container.image }} before recreating, so the new container runs the latest push of that tag.</span></span>
      </label>

      <template v-if="kind === 'remove'">
        <label class="flex cursor-pointer items-start gap-2 text-sm">
          <Checkbox :model-value="removeVolumes" class="mt-0.5" data-testid="action-remove-volumes" @update:model-value="removeVolumes = $event === true" />
          <span><span class="text-foreground">Also remove its anonymous volumes</span><span class="block text-xs text-sev-warn"><TriangleAlert class="mr-1 inline size-3" />Data in them is gone for good. Named volumes are never removed here.</span></span>
        </label>
        <FormField :label="`Type ${container.name} to confirm`" for="action-typed">
          <Input id="action-typed" v-model="typed" data-testid="action-typed" autocomplete="off" class="font-mono" />
        </FormField>
      </template>

      <DialogFooter>
        <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <Button :variant="meta.destructive ? 'destructive' : 'default'" :disabled="!canSubmit || pending" data-testid="action-submit" @click="submit">
          <Loader2 v-if="pending" class="animate-spin" />
          {{ offline ? `Queue ${meta.label.toLowerCase()}` : meta.label }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
