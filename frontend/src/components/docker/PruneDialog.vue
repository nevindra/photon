<script setup lang="ts">
// Prune with a preview: what would be freed, per category, before anything is deleted. Images,
// stopped containers and build cache are on by default — they are recreatable. Volumes are off,
// carry a data-loss line, and turn the confirmation into a typed one: an unused volume is
// sometimes last month's database.
import { computed, ref, watch } from 'vue'
import { Loader2, TriangleAlert } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { Spinner } from '@/components/ui/spinner'
import { usePrunePreview } from '@/lib/deploy/deployQueries'
import { pruneSummary } from '@/lib/deploy/actions'
import { formatBytes, formatNumber } from '@/lib/core/format'
import type { ActionArgs, AgentStatus, PruneCategory } from '@/lib/core/api'

const props = defineProps<{ open: boolean; host: string; hostStatus: AgentStatus; pending?: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; confirm: [args: ActionArgs] }>()

const previewQ = usePrunePreview(() => props.host, () => props.open)
const preview = computed(() => previewQ.data.value ?? null)

const sel = ref({ images: true, containers: true, build_cache: true, volumes: false })
const typed = ref('')
watch(() => props.open, (o) => { if (o) { sel.value = { images: true, containers: true, build_cache: true, volumes: false }; typed.value = '' } })

type Key = keyof typeof sel.value
const CATS: { key: Key; label: string; what: string }[] = [
  { key: 'images', label: 'Images', what: 'not used by any container' },
  { key: 'containers', label: 'Stopped containers', what: 'exited, dead or never started' },
  { key: 'build_cache', label: 'Build cache', what: 'layers from local builds' },
  { key: 'volumes', label: 'Volumes', what: 'attached to no container — their data is deleted' },
]
const cat = (k: Key): PruneCategory => preview.value?.[k] ?? { count: 0, bytes: 0 }
const summary = computed(() => pruneSummary(CATS.map((c) => ({ label: c.label, bytes: cat(c.key).bytes, count: cat(c.key).count, selected: sel.value[c.key] })), formatBytes))
const total = computed(() => CATS.reduce((n, c) => n + (sel.value[c.key] ? cat(c.key).bytes : 0), 0))
const anything = computed(() => CATS.some((c) => sel.value[c.key] && (cat(c.key).count > 0 || cat(c.key).bytes > 0)))
const needsTyped = computed(() => sel.value.volumes && cat('volumes').count > 0)
const canSubmit = computed(() => anything.value && (!needsTyped.value || typed.value === 'prune'))
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-w-lg" data-testid="prune-dialog">
      <DialogHeader>
        <DialogTitle>Prune {{ host }}</DialogTitle>
        <DialogDescription>Remove what nothing uses. Pick the categories; the preview says what each would free before anything is deleted.</DialogDescription>
      </DialogHeader>

      <div v-if="!preview" class="flex items-center justify-center py-8"><Spinner size="md">Measuring…</Spinner></div>
      <div v-else class="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface-1">
        <label v-for="c in CATS" :key="c.key" class="flex cursor-pointer items-start gap-3 px-3 py-2.5" :data-prune="c.key">
          <Checkbox :model-value="sel[c.key]" class="mt-0.5" :disabled="!cat(c.key).count && !cat(c.key).bytes" @update:model-value="sel[c.key] = $event === true" />
          <span class="min-w-0 flex-1">
            <span class="flex items-baseline justify-between gap-2 text-sm">
              <span class="text-foreground">{{ c.label }} <span class="text-xs text-muted-foreground">· {{ formatNumber(cat(c.key).count) }}</span></span>
              <span class="font-mono text-xs tabular-nums" :class="cat(c.key).bytes ? 'text-foreground' : 'text-muted-foreground/60'">{{ formatBytes(cat(c.key).bytes) }}</span>
            </span>
            <span class="block text-xs" :class="c.key === 'volumes' ? 'text-sev-warn' : 'text-muted-foreground'">
              <TriangleAlert v-if="c.key === 'volumes'" class="mr-1 inline size-3" />{{ c.what }}
            </span>
          </span>
        </label>
      </div>

      <p class="text-sm" :class="total ? 'text-foreground' : 'text-muted-foreground'" data-testid="prune-summary">{{ summary }}</p>

      <FormField v-if="needsTyped" label="Type prune to confirm deleting volumes" for="prune-typed">
        <Input id="prune-typed" v-model="typed" data-testid="prune-typed" autocomplete="off" class="font-mono" />
      </FormField>

      <p v-if="hostStatus !== 'online'" class="text-xs text-muted-foreground">The host is offline — the prune waits as pending and runs on the next sync, or expires in 15 minutes.</p>

      <DialogFooter>
        <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <Button :variant="sel.volumes ? 'destructive' : 'default'" :disabled="!canSubmit || pending" data-testid="prune-submit" @click="emit('confirm', { ...sel })">
          <Loader2 v-if="pending" class="animate-spin" />
          {{ total ? `Free ${formatBytes(total)}` : 'Prune' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
