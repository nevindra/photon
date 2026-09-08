<script setup lang="ts">
// One confirmation for the stack operations. Apply / restart / redeploy state what will happen
// to which host in one sentence and ask once; down asks once; destroy wants the name typed and
// carries the volumes choice; reconcile lists exactly what will be reverted, because reverting
// what somebody changed at 2am to keep production up is a decision, not a click.
import { computed, ref, watch } from 'vue'
import { Loader2, TriangleAlert, WifiOff } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { FormField } from '@/components/ui/form-field'
import { driftLabel } from '@/lib/deploy/stacks'
import type { AgentStatus, Stack, StackActionArgs, StackActionKind } from '@/lib/core/api'

const props = defineProps<{ open: boolean; kind: StackActionKind | null; stack: Stack | null; hostStatus: AgentStatus; pending?: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; confirm: [args: StackActionArgs] }>()

const typed = ref('')
const volumes = ref(false)
const pull = ref(false)
watch(() => props.open, (o) => { if (o) { typed.value = ''; volumes.value = false; pull.value = props.kind === 'redeploy' } })

const offline = computed(() => props.hostStatus !== 'online')
const META = computed(() => {
  const s = props.stack
  if (!s || !props.kind) return null
  const gen = s.desired_generation
  switch (props.kind) {
    case 'up':
      return { title: `Apply ${s.name} on ${s.host}`, body: `Runs compose up for generation ${gen} — pull ${s.deploy_options.pull}${s.deploy_options.force_recreate ? ', force recreate' : ''}${s.deploy_options.recreate_volumes ? ', recreate anonymous volumes' : ''}. Services whose configuration changed are recreated; the rest are left running.`, verb: 'Apply', destructive: false }
    case 'restart':
      return { title: `Restart ${s.name} on ${s.host}`, body: `Restarts every service in the stack (${s.services.map((x) => x.name).join(', ')}). A short gap on each published port.`, verb: 'Restart', destructive: false }
    case 'redeploy':
      return { title: `Redeploy ${s.name} on ${s.host}`, body: `Pulls every image, then recreates the services whose image changed. This is how a new tag reaches the host.`, verb: 'Redeploy', destructive: false }
    case 'down':
      return { title: `Take ${s.name} down on ${s.host}`, body: `Stops and removes the stack's containers and networks. Volumes are kept. The stack stays declared here, so Apply brings it back.`, verb: 'Take down', destructive: true }
    case 'destroy':
      return { title: `Destroy ${s.name} on ${s.host}`, body: `Removes the stack from the host and from management. Its history stays in the audit log.`, verb: 'Destroy', destructive: true }
    case 'accept':
      return { title: `Accept what runs on ${s.host}?`, body: `Makes the ${s.drift.length} observed ${s.drift.length === 1 ? 'difference' : 'differences'} the new desired state: generation ${gen + 1} is rendered from what runs, the drift clears, and the change is on record as yours.`, verb: 'Accept as desired state', destructive: false }
    case 'reconcile':
      return { title: `Reconcile ${s.name} on ${s.host}`, body: `Reverts what runs to generation ${s.applied_generation ?? gen} as Photon applied it. Somebody changed these by hand; make sure they meant to lose it.`, verb: 'Revert to desired state', destructive: true }
  }
})
const canSubmit = computed(() => !!META.value && (props.kind !== 'destroy' || typed.value === props.stack?.name))
function submit(): void {
  if (!canSubmit.value) return
  const args: StackActionArgs = {}
  if (props.kind === 'destroy') args.volumes = volumes.value
  if (props.kind === 'redeploy' || props.kind === 'up') args.pull = pull.value
  emit('confirm', args)
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent v-if="META && stack && kind" class="max-w-md" data-testid="stack-action-dialog" :data-kind="kind">
      <DialogHeader>
        <DialogTitle>{{ META.title }}</DialogTitle>
        <DialogDescription>{{ META.body }}</DialogDescription>
      </DialogHeader>

      <ul v-if="(kind === 'reconcile' || kind === 'accept') && stack.drift.length" class="flex flex-col divide-y divide-border rounded-lg border border-sev-warn/30 bg-card text-xs" data-testid="reconcile-list">
        <li v-for="(d, i) in stack.drift" :key="i" class="px-3 py-2">
          <span class="font-medium text-sev-warn">{{ driftLabel(d) }}</span><span v-if="d.service" class="ml-1 font-mono text-muted-foreground">{{ d.service }}<template v-if="d.field">.{{ d.field }}</template></span>
          <div v-if="kind === 'reconcile'" class="mt-0.5 grid grid-cols-[3.5rem_1fr] gap-x-2 font-mono"><span class="text-muted-foreground">runs</span><span class="text-foreground line-through decoration-sev-warn/60">{{ d.observed ?? '—' }}</span><span class="text-muted-foreground">back to</span><span class="text-foreground">{{ d.desired ?? '—' }}</span></div>
          <div v-else class="mt-0.5 grid grid-cols-[3.5rem_1fr] gap-x-2 font-mono"><span class="text-muted-foreground">was</span><span class="text-foreground line-through decoration-muted-foreground/60">{{ d.desired ?? '—' }}</span><span class="text-muted-foreground">keeps</span><span class="text-foreground">{{ d.observed ?? '—' }}</span></div>
        </li>
      </ul>

      <div v-if="offline" class="flex items-start gap-2 rounded-lg border border-border bg-muted px-3 py-2 text-xs" data-testid="stack-offline-note">
        <WifiOff class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        <span class="text-foreground">{{ stack.host }} is {{ hostStatus === 'offline' ? 'offline' : 'waiting for its first sync' }}. This waits as <b>pending</b> and applies on the agent's next sync — it does not fail.</span>
      </div>

      <label v-if="kind === 'up'" class="flex cursor-pointer items-start gap-2 text-sm">
        <Checkbox :model-value="pull" class="mt-0.5" @update:model-value="pull = $event === true" />
        <span class="text-foreground">Pull images first<span class="block text-xs text-muted-foreground">Re-pull every tag even if the host has it.</span></span>
      </label>

      <template v-if="kind === 'destroy'">
        <label class="flex cursor-pointer items-start gap-2 text-sm">
          <Checkbox :model-value="volumes" class="mt-0.5" data-testid="destroy-volumes" @update:model-value="volumes = $event === true" />
          <span><span class="text-foreground">Also delete its named volumes</span><span class="block text-xs text-sev-warn"><TriangleAlert class="mr-1 inline size-3" />The data in them is gone for good. Off, the volumes stay on the host for a future stack to reuse.</span></span>
        </label>
        <FormField :label="`Type ${stack.name} to confirm`" for="destroy-typed">
          <Input id="destroy-typed" v-model="typed" data-testid="destroy-typed" autocomplete="off" class="font-mono" />
        </FormField>
      </template>

      <DialogFooter>
        <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <Button :variant="META.destructive ? 'destructive' : 'default'" :disabled="!canSubmit || pending" data-testid="stack-action-submit" @click="submit">
          <Loader2 v-if="pending" class="animate-spin" />{{ offline ? `Queue: ${META.verb.toLowerCase()}` : META.verb }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
