<script setup lang="ts">
// Remove one named volume. This is the `typed` tier of the confirmation policy: a volume IS
// data (a database, a model pack), and there is no undo, so the name is typed and the size is
// stated. Only offered for a volume nothing mounts; the server refuses the rest anyway.
import { computed, ref, watch } from 'vue'
import { Loader2, WifiOff, TriangleAlert } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { formatBytes } from '@/lib/core/format'
import type { AgentStatus, VolumeInfo } from '@/lib/core/api'

const props = defineProps<{ open: boolean; volume: VolumeInfo | null; hostStatus: AgentStatus; pending?: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; confirm: [] }>()

const typed = ref('')
watch(
  () => props.open,
  (o) => {
    if (o) typed.value = ''
  },
)
const offline = computed(() => props.hostStatus !== 'online')
const canSubmit = computed(() => !!props.volume && typed.value === props.volume.name)
function submit(): void {
  if (canSubmit.value) emit('confirm')
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent v-if="volume" class="max-w-md" data-testid="volume-remove">
      <DialogHeader>
        <DialogTitle>Remove volume</DialogTitle>
        <p class="break-all font-mono text-sm text-foreground" data-testid="volume-remove-name">{{ volume.name }}</p>
        <DialogDescription>
          Deletes the volume and everything in it{{ volume.size_bytes == null ? '' : ` — ${formatBytes(volume.size_bytes)}` }}. A stack that mounts it again gets an empty volume under the same name.
        </DialogDescription>
      </DialogHeader>

      <p class="flex items-start gap-2 text-xs text-sev-warn"><TriangleAlert class="mt-0.5 size-3.5 shrink-0" /><span>The data is gone for good. Photon keeps no copy.</span></p>

      <div v-if="offline" class="flex items-start gap-2 rounded-lg border border-border bg-muted px-3 py-2 text-xs" data-testid="volume-offline-note">
        <WifiOff class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        <span class="text-foreground">This host is {{ hostStatus === 'offline' ? 'offline' : 'waiting for its first sync' }}. The action waits as <b>pending</b> and runs on the agent's next sync — or expires in 15 minutes.</span>
      </div>

      <FormField :label="`Type ${volume.name} to confirm`" for="volume-typed">
        <Input id="volume-typed" v-model="typed" data-testid="volume-typed" autocomplete="off" class="font-mono" @keydown.enter.prevent="submit" />
      </FormField>

      <DialogFooter>
        <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <Button variant="destructive" :disabled="!canSubmit || pending" data-testid="volume-remove-submit" @click="submit">
          <Loader2 v-if="pending" class="animate-spin" />
          {{ offline ? 'Queue remove' : 'Remove' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
