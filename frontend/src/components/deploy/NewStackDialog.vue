<script setup lang="ts">
// Create a stack from a compose file (admin — arbitrary compose is the one path that can run
// anything), or adopt a compose project already running on a host (operator). A template deploy
// is the third path and lives on the Templates catalog, where the recipe is.
//
// The editor lints as you type and refuses to submit until every risky construct is
// acknowledged; the deploy options sit under the files; the button says what will happen to
// which host.
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Loader2, Server, Import, ChevronRight } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { SelectMenu } from '@/components/ui/select-menu'
import { Switch } from '@/components/ui/switch'
import { Badge } from '@/components/ui/badge'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import ComposeEditor from './ComposeEditor.vue'
import DeployOptionsForm from './DeployOptionsForm.vue'
import GatedButton from './GatedButton.vue'
import { useAgents, useCreateStack, useDiscoveredProjects, useAdoptProject } from '@/lib/deploy/deployQueries'
import { DEFAULT_COMPOSE, DEFAULT_ENV } from '@/lib/deploy/compose'
import { AGENT_STATUS_META } from '@/lib/deploy/agentStatus'
import { can } from '@/lib/core/auth'
import { cn } from '@/lib/core/utils'
import type { DeployOptions, StackFile } from '@/lib/core/api'

const props = defineProps<{ open: boolean; mode?: 'compose' | 'adopt'; prefillHost?: string | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const router = useRouter()
const agentsQ = useAgents()
// Only hosts whose agent may apply; the others are listed disabled with the reason.
const hostOptions = computed(() => (agentsQ.data.value?.hosts ?? []).map((h) => ({
  value: h.host,
  label: `${h.host}${h.site ? ` · ${h.site}` : ''}${!h.capabilities.includes('apply') ? ' (no apply)' : h.status !== 'online' ? ` (${AGENT_STATUS_META[h.status].label.toLowerCase()})` : ''}`,
})))
const agentFor = (host: string) => (agentsQ.data.value?.hosts ?? []).find((h) => h.host === host) ?? null

const mode = ref<'compose' | 'adopt'>('compose')
const name = ref('')
const host = ref('')
const files = ref<StackFile[]>([])
const acknowledged = ref<string[]>([])
const options = ref<DeployOptions>({ pull: 'missing', force_recreate: false, recreate_volumes: false })
const applyNow = ref(true)
const error = ref('')
const editor = ref<InstanceType<typeof ComposeEditor> | null>(null)

watch(
  () => props.open,
  (o) => {
    if (!o) return
    mode.value = props.mode ?? (can('compose') ? 'compose' : 'adopt')
    name.value = ''
    host.value = props.prefillHost ?? ''
    files.value = [{ path: 'compose.yaml', content: DEFAULT_COMPOSE }, { path: '.env', content: DEFAULT_ENV }]
    acknowledged.value = []
    options.value = { pull: 'missing', force_recreate: false, recreate_volumes: false }
    applyNow.value = true
    error.value = ''
  },
  { immediate: true },
)

const create = useCreateStack()
const hostAgent = computed(() => (host.value ? agentFor(host.value) : null))
// Writing compose needs `apply`; adopting recreates nothing, so any managed host can be read.
const hostBlocked = computed(() => (mode.value === 'compose' && hostAgent.value && !hostAgent.value.capabilities.includes('apply') ? 'The agent on this host was installed without --docker-apply.' : null))
const adoptNote = computed(() => (mode.value === 'adopt' && hostAgent.value && !hostAgent.value.capabilities.includes('apply') ? 'This agent has no --docker-apply: the project is watched for drift, but applying changes from here needs a reinstall with it.' : null))
const hostHint = computed(() => hostAgent.value && hostAgent.value.status !== 'online' ? `${hostAgent.value.host} is ${AGENT_STATUS_META[hostAgent.value.status].label.toLowerCase()} — the stack waits as pending and applies on its next sync.` : mode.value === 'adopt' ? 'Any managed host; adopting reads the project without recreating anything.' : 'A managed host whose agent can apply stacks.')
// The folded Deploy options row says what it holds, so nobody opens it just to check.
const optionsSummary = computed(() => [`pull ${options.value.pull}`, options.value.force_recreate ? 'force recreate' : null, options.value.recreate_volumes ? 'recreate anonymous volumes' : null].filter((x): x is string => !!x))
const canSubmit = computed(() => !!name.value.trim() && !!host.value && !hostBlocked.value && !!editor.value?.ready)

async function submit(): Promise<void> {
  error.value = ''
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(name.value.trim())) {
    error.value = 'Lowercase letters, digits, - or _ — it becomes the compose project name on the host.'
    return
  }
  const res = await create.mutateAsync({ name: name.value.trim(), host: host.value, files: files.value, deploy_options: options.value, acknowledged_warnings: acknowledged.value, apply: applyNow.value })
  if (res.ok && res.stack) {
    emit('update:open', false)
    router.push(`/deploy/stacks/${encodeURIComponent(res.stack.id)}`)
  } else error.value = res.error ?? 'Could not create the stack.'
}

// --- Adopt ---
const discoverQ = useDiscoveredProjects(() => (mode.value === 'adopt' && host.value ? host.value : null))
const projects = computed(() => discoverQ.data.value?.projects ?? [])
const adopt = useAdoptProject()
async function doAdopt(project: string): Promise<void> {
  const res = await adopt.mutateAsync({ host: host.value, project })
  if (res.ok && res.stack) {
    emit('update:open', false)
    router.push(`/deploy/stacks/${encodeURIComponent(res.stack.id)}`)
  }
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <!-- A flex column: the header, rows and editor scroll; the action row below stays put. -->
    <DialogContent class="flex max-h-[92vh] max-w-5xl flex-col gap-0 overflow-hidden p-0" data-testid="new-stack-dialog">
      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-6 [&>*]:min-w-0">
        <div class="flex flex-wrap items-start justify-between gap-x-6 gap-y-3 pr-8">
          <DialogHeader class="flex-1 basis-[28rem]">
            <DialogTitle>{{ mode === 'adopt' ? 'Adopt a running project' : 'New stack' }}</DialogTitle>
            <DialogDescription>
              {{ mode === 'adopt' ? 'Take a compose project somebody already started on a host under management. Nothing is recreated: Photon records what runs and watches it for drift from here on.' : 'A compose file and its .env, applied on one host with compose semantics. The folder is the source of truth; every save is a new generation.' }}
            </DialogDescription>
          </DialogHeader>
          <Segmented v-model="mode" data-testid="new-stack-mode" class="w-fit shrink-0">
            <SegmentedItem value="compose" :disabled="!can('compose')" :title="can('compose') ? undefined : 'Arbitrary compose needs the Admin role'">Write compose</SegmentedItem>
            <SegmentedItem value="adopt">Adopt existing</SegmentedItem>
          </Segmented>
        </div>

        <!-- Where: a settings sheet — label and hint left, control right, one choice per row. -->
        <div class="flex flex-col">
          <div v-if="mode === 'compose'" class="grid gap-1.5 pb-3 sm:grid-cols-[minmax(0,200px)_minmax(0,1fr)] sm:gap-x-7">
            <div class="flex flex-col gap-0.5 sm:pt-2">
              <Label for="stack-name" class="text-[13.5px] leading-snug">Stack name</Label>
              <p class="text-xs leading-relaxed text-muted-foreground">Becomes the compose project name on the host.</p>
            </div>
            <div class="max-w-sm"><Input id="stack-name" v-model="name" data-testid="stack-name" placeholder="nodeflux-vas" autocomplete="off" class="font-mono" /></div>
          </div>
          <div :class="cn('grid gap-1.5 py-3 sm:grid-cols-[minmax(0,200px)_minmax(0,1fr)] sm:gap-x-7', mode === 'compose' && 'border-t border-border')">
            <div class="flex flex-col gap-0.5 sm:pt-2">
              <Label for="stack-host" class="text-[13.5px] leading-snug">Host</Label>
              <p class="text-xs leading-relaxed text-muted-foreground">{{ hostHint }}</p>
            </div>
            <div class="flex flex-col gap-1.5">
              <div class="max-w-sm"><SelectMenu id="stack-host" :model-value="host" :options="[{ value: '', label: 'Choose a host…' }, ...hostOptions]" content-class="w-72" size="md" aria-label="Host" @update:model-value="host = String($event)" /></div>
              <p v-if="hostBlocked" class="font-mono text-xs text-sev-error">{{ hostBlocked }}</p>
            </div>
          </div>
        </div>

        <template v-if="mode === 'compose'">
          <ComposeEditor ref="editor" :files="files" :acknowledged="acknowledged" :server-error="error || null" @update:files="files = $event" @update:acknowledged="acknowledged = $event" />
          <!-- Three settings most stacks never change: folded, with the current values in the summary. -->
          <details class="group rounded-xl border border-border bg-card" data-testid="deploy-options-fold">
            <summary class="flex cursor-pointer list-none items-center gap-2.5 px-3.5 py-2.5 text-sm [&::-webkit-details-marker]:hidden">
              <ChevronRight class="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
              <span class="font-medium text-foreground">Deploy options</span>
              <span class="flex flex-wrap gap-1.5">
                <Badge v-for="o in optionsSummary" :key="o" variant="outline" class="font-mono text-[11px] font-normal text-muted-foreground">{{ o }}</Badge>
              </span>
            </summary>
            <div class="border-t border-border bg-muted/30 px-3.5 py-3 sm:pl-10"><DeployOptionsForm v-model="options" /></div>
          </details>
        </template>

        <template v-else>
          <p v-if="adoptNote" class="text-xs text-muted-foreground">{{ adoptNote }}</p>
          <div v-if="!host" class="rounded-xl border border-border bg-card py-6"><EmptyState title="Pick a host" description="Photon lists the compose projects running there that it does not manage yet." class="py-4" /></div>
          <div v-else-if="discoverQ.isLoading.value" class="flex justify-center py-8"><Spinner size="md">Looking at {{ host }}…</Spinner></div>
          <div v-else-if="!projects.length" class="rounded-xl border border-border bg-card py-6"><EmptyState title="Nothing to adopt" :description="`Every compose project on ${host} is already managed, or there are none.`" class="py-4" /></div>
          <ul v-else class="flex flex-col divide-y divide-border overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="adoptable">
            <li v-for="p in projects" :key="p.name" class="flex flex-wrap items-center gap-3 px-4 py-3" :data-project="p.name">
              <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-2"><span class="font-medium text-foreground">{{ p.name }}</span><Badge variant="outline"><Server class="size-3" /> {{ p.host }}</Badge><span class="text-xs text-muted-foreground">{{ p.running }} / {{ p.containers }} running</span></div>
                <div class="mt-0.5 font-mono text-xs text-muted-foreground">{{ p.services.join(', ') }}<span v-if="p.compose_file"> · {{ p.compose_file }}</span></div>
              </div>
              <GatedButton perm="deploy" size="sm" variant="outline" :disabled="adopt.isPending.value || !!hostBlocked" :title="hostBlocked ?? undefined" :data-adopt="p.name" @click="doAdopt(p.name)"><Import class="size-4" /> Adopt</GatedButton>
            </li>
          </ul>
        </template>
      </div>

      <div class="flex shrink-0 flex-wrap items-center justify-between gap-3 border-t border-border bg-surface-2 px-6 py-3" data-testid="new-stack-actions">
        <label v-if="mode === 'compose'" class="flex items-center gap-2 text-sm" for="apply-now">
          <Switch id="apply-now" :model-value="applyNow" data-testid="apply-now" @update:model-value="applyNow = $event" />
          <span class="text-foreground">Apply right away</span>
          <span class="text-xs text-muted-foreground">{{ applyNow ? '' : 'Off: save and render only; apply later from the stack page.' }}</span>
        </label>
        <span v-else class="text-xs text-muted-foreground">Adopting records what runs; nothing is recreated.</span>
        <div class="flex items-center gap-2">
          <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
          <GatedButton v-if="mode === 'compose'" perm="compose" :disabled="!canSubmit || create.isPending.value" data-testid="stack-submit" @click="submit">
            <Loader2 v-if="create.isPending.value" class="animate-spin" />
            {{ applyNow ? (host ? `Create and apply on ${host}` : 'Create and apply') : 'Save without applying' }}
          </GatedButton>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
