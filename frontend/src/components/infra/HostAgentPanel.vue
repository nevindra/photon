<script setup lang="ts">
// The Agent tab of `/infra/:host`: is this machine under management, what may the agent do, is it
// talking to us, and the admin controls (rotate token, poll interval, upgrade, remove).
//
// Capabilities are shown as REPORTED — the agent admits to them on every sync — and the copy says
// they cannot be toggled from here. That is R1.3 enforced at the edge: a customer who allowed
// read-only gets read-only, and the UI does not pretend otherwise with a switch that would do
// nothing.
import { computed, ref, watch } from 'vue'
import { KeyRound, ArrowUpCircle, Trash2, Check, Minus, Loader2, Plus } from 'lucide-vue-next'
import { Card } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { StatusPill } from '@/components/ui/status-pill'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { NumberField } from '@/components/ui/number-field'
import { FormField } from '@/components/ui/form-field'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import GatedButton from '@/components/deploy/GatedButton.vue'
import AddHostDialog from '@/components/infra/AddHostDialog.vue'
import AgentTokenPanel from '@/components/infra/AgentTokenPanel.vue'
import DockerDiskBar from '@/components/infra/DockerDiskBar.vue'
import { useAgent, useRotateAgentToken, useUpdateAgent, useDeleteAgent, useRequestAgentUpgrade } from '@/lib/deploy/deployQueries'
import { AGENT_STATUS_META, CAPABILITIES, CAPABILITY_META, upgradeAvailable } from '@/lib/deploy/agentStatus'
import { ageMs, stampMs } from '@/lib/deploy/audit'
import { formatNumber } from '@/lib/core/format'
import { useRouter } from 'vue-router'
import type { MutationResult, TokenIssue } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'

const props = defineProps<{ host: string }>()

const router = useRouter()
const q = useAgent(() => props.host)
const agent = computed(() => q.data.value ?? null)
const loading = computed(() => q.isLoading.value)
const status = computed(() => (agent.value ? AGENT_STATUS_META[agent.value.status] : null))

const enrollOpen = ref(false)
const rotate = useRotateAgentToken()
const update = useUpdateAgent()
const remove = useDeleteAgent()
const upgrade = useRequestAgentUpgrade()

const rotated = ref<{ token: string; serverUrl: string | null; archs: string[] | null } | null>(null)
async function doRotate(): Promise<void> {
  const res: MutationResult & TokenIssue = await rotate.mutateAsync(props.host)
  if (res.ok && res.token) rotated.value = { token: res.token, serverUrl: res.server_url ?? null, archs: res.agent_archs ?? null }
}

const pollDraft = ref(15)
watch(agent, (a) => { if (a) pollDraft.value = a.poll_secs }, { immediate: true })
const pollDirty = computed(() => !!agent.value && pollDraft.value !== agent.value.poll_secs)
async function savePoll(): Promise<void> {
  await update.mutateAsync({ host: props.host, patch: { poll_secs: pollDraft.value } })
}

const removeOpen = ref(false)
const removeConfirm = ref('')
async function doRemove(): Promise<void> {
  const res = await remove.mutateAsync(props.host)
  if (res.ok) {
    removeOpen.value = false
    router.push('/infra')
  }
}

const labels = computed(() => Object.entries(agent.value?.labels ?? {}))
</script>

<template>
  <DeployUnavailable v-if="isDeployUnavailable(q.error.value)" what="Agent management" />
  <div v-else-if="loading && !agent" class="flex flex-1 items-center justify-center py-16">
    <Spinner size="lg">Loading agent…</Spinner>
  </div>

  <!-- Not enrolled -->
  <Card v-else-if="!agent" class="flex flex-col items-center py-4" data-testid="agent-not-enrolled">
    <EmptyState
      title="No agent manages this host"
      description="Metrics can arrive from a plain photon-agent. Docker management needs the agent enrolled here with a Docker capability."
      class="py-10"
    >
      <GatedButton perm="agents" class="mt-3" data-testid="enroll-host" @click="enrollOpen = true"><Plus class="size-4" /> Enroll {{ host }}</GatedButton>
    </EmptyState>
    <AddHostDialog :open="enrollOpen" :prefill-host="host" @update:open="enrollOpen = $event" />
  </Card>

  <div v-else class="flex flex-col gap-4" data-testid="agent-panel">
    <!-- Status header -->
    <div class="grid gap-4 lg:grid-cols-3">
      <Card class="p-4">
        <div class="flex items-center justify-between gap-2">
          <p class="text-[13px] font-medium text-foreground">Connection</p>
          <StatusPill :tone="status!.tone" :solid="agent.status === 'offline'" data-testid="agent-status">{{ status!.label }}</StatusPill>
        </div>
        <p class="mt-1 text-xs text-muted-foreground">{{ status!.hint }}</p>
        <dl class="mt-3 grid grid-cols-2 gap-x-4 gap-y-1.5 text-xs">
          <dt class="text-muted-foreground">Last sync</dt>
          <dd class="tabular-nums text-foreground" :title="agent.last_seen_ms ? new Date(agent.last_seen_ms).toISOString() : undefined">{{ agent.last_seen_ms ? ageMs(agent.last_seen_ms) : 'never' }}</dd>
          <dt class="text-muted-foreground">Polls every</dt>
          <dd class="tabular-nums text-foreground">{{ agent.poll_secs }}s</dd>
          <dt class="text-muted-foreground">Site</dt>
          <dd class="text-foreground">{{ agent.site ?? 'this Photon' }}</dd>
          <dt class="text-muted-foreground">Enrolled</dt>
          <dd class="text-foreground">{{ stampMs(agent.enrolled_ms) }}</dd>
        </dl>
      </Card>

      <Card class="p-4">
        <div class="flex items-center justify-between gap-2">
          <p class="text-[13px] font-medium text-foreground">Agent version</p>
          <Badge v-if="upgradeAvailable(agent)" variant="brand" data-testid="agent-upgrade-badge">{{ agent.latest_agent_version }} available</Badge>
          <Badge v-else-if="agent.agent_version" variant="success">Up to date</Badge>
        </div>
        <p class="mt-2 font-mono text-2xl font-semibold tracking-tight text-foreground">{{ agent.agent_version ?? '—' }}</p>
        <p class="mt-1 text-xs text-muted-foreground">
          <template v-if="upgradeAvailable(agent)">Upgrades ride the sync channel: the agent fetches the new binary from this Photon, verifies it, swaps and restarts itself. No SSH needed.</template>
          <template v-else-if="!agent.agent_version">Reported on the first sync.</template>
          <template v-else>Matches what this Photon serves.</template>
        </p>
        <GatedButton v-if="upgradeAvailable(agent)" perm="agents" size="sm" variant="outline" class="mt-3" :disabled="upgrade.isPending.value" data-testid="agent-upgrade" @click="upgrade.mutate(host)">
          <ArrowUpCircle class="size-4" /> Upgrade on next sync
        </GatedButton>
      </Card>

      <Card class="p-4">
        <p class="text-[13px] font-medium text-foreground">Capabilities</p>
        <p class="mt-1 text-xs text-muted-foreground">As reported by the agent. Set on the host with flags — Photon cannot widen them from here.</p>
        <ul class="mt-3 flex flex-col gap-1.5" data-testid="agent-capabilities">
          <li v-for="cap in CAPABILITIES" :key="cap" class="flex items-center gap-2 text-xs" :data-cap="cap" :data-granted="agent.capabilities.includes(cap)">
            <Check v-if="agent.capabilities.includes(cap)" class="size-3.5 text-success" />
            <Minus v-else class="size-3.5 text-muted-foreground/40" />
            <span :class="agent.capabilities.includes(cap) ? 'text-foreground' : 'text-muted-foreground/70'">{{ CAPABILITY_META[cap].label }}</span>
            <code class="ml-auto font-mono text-[10px] text-muted-foreground/70">{{ CAPABILITY_META[cap].flag }}</code>
          </li>
        </ul>
      </Card>
    </div>

    <!-- Docker facts -->
    <Card v-if="agent.docker" class="p-4" data-testid="agent-docker">
      <div class="flex flex-wrap items-baseline justify-between gap-2">
        <p class="text-[13px] font-medium text-foreground">Docker</p>
        <p class="text-xs text-muted-foreground">
          {{ agent.docker.version }} · API {{ agent.docker.api_version }} · {{ agent.docker.storage_driver }} · {{ agent.docker.os }} / {{ agent.docker.arch }}
        </p>
      </div>
      <div class="mt-3 grid grid-cols-3 gap-3 text-xs">
        <div><p class="text-muted-foreground">Containers</p><p class="mt-0.5 text-lg font-semibold tabular-nums text-foreground">{{ formatNumber(agent.docker.containers_running) }}<span class="text-sm font-normal text-muted-foreground"> / {{ formatNumber(agent.docker.containers_total) }} running</span></p></div>
        <div><p class="text-muted-foreground">Images</p><p class="mt-0.5 text-lg font-semibold tabular-nums text-foreground">{{ formatNumber(agent.docker.images) }}</p></div>
        <div><p class="text-muted-foreground">Labels</p><p class="mt-1 flex flex-wrap gap-1"><Badge v-for="[k, v] in labels" :key="k" variant="outline" class="font-mono">{{ k }}={{ v }}</Badge><span v-if="!labels.length" class="text-muted-foreground/60">none</span></p></div>
      </div>
      <div class="mt-4 border-t border-border pt-3">
        <DockerDiskBar :disk="agent.docker.disk" />
      </div>
    </Card>
    <Card v-else class="p-4 text-xs text-muted-foreground" data-testid="agent-docker-unknown">
      Docker facts arrive with the first sync — daemon version, disk used by images and volumes, container counts.
    </Card>

    <!-- Admin -->
    <Card class="divide-y divide-border p-0" data-testid="agent-admin">
      <div class="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
        <div>
          <p class="text-sm font-medium text-foreground">Poll interval</p>
          <p class="text-xs text-muted-foreground">Server-driven and adaptive: while a log view is open it drops to 1s, then returns to this.</p>
        </div>
        <div class="flex items-center gap-2">
          <NumberField v-model="pollDraft" :min="15" :max="3600" unit="s" class="w-28" />
          <GatedButton perm="agents" size="sm" variant="outline" :disabled="!pollDirty || update.isPending.value" data-testid="agent-save-poll" @click="savePoll">Save</GatedButton>
        </div>
      </div>
      <div class="flex flex-col gap-3 px-4 py-3">
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div>
            <p class="text-sm font-medium text-foreground">Agent token</p>
            <p class="text-xs text-muted-foreground">Per host, minted here, never reused from ingest. Rotate it if it may have leaked; the old one stops working on the next sync.</p>
          </div>
          <GatedButton perm="agents" size="sm" variant="outline" :disabled="rotate.isPending.value" data-testid="agent-rotate" @click="doRotate"><KeyRound class="size-4" /> Rotate token</GatedButton>
        </div>
        <AgentTokenPanel v-if="rotated" :token="rotated.token" :host="host" :capabilities="agent.capabilities" :poll-secs="agent.poll_secs" :server-url="rotated.serverUrl" :agent-archs="rotated.archs" rotated />
      </div>
      <div class="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
        <div>
          <p class="text-sm font-medium text-foreground">Remove from management</p>
          <p class="text-xs text-muted-foreground">Revokes the token and forgets the host. Nothing on the machine is stopped — its containers keep running, and the agent goes idle.</p>
        </div>
        <GatedButton perm="agents" size="sm" variant="outline" class="text-sev-error hover:text-sev-error" data-testid="agent-remove" @click="removeOpen = true; removeConfirm = ''"><Trash2 class="size-4" /> Remove agent</GatedButton>
      </div>
    </Card>

    <Dialog :open="removeOpen" @update:open="removeOpen = $event">
      <DialogContent class="max-w-md">
        <DialogHeader>
          <DialogTitle>Remove {{ host }} from management?</DialogTitle>
          <DialogDescription>The agent's token is revoked and Photon forgets this host's stacks and desired state. Containers on the machine keep running. Type the host name to confirm.</DialogDescription>
        </DialogHeader>
        <FormField :label="`Type ${host}`" for="remove-confirm">
          <Input id="remove-confirm" v-model="removeConfirm" data-testid="agent-remove-confirm" autocomplete="off" />
        </FormField>
        <DialogFooter>
          <Button variant="ghost" @click="removeOpen = false">Cancel</Button>
          <Button variant="destructive" :disabled="removeConfirm !== host || remove.isPending.value" data-testid="agent-remove-submit" @click="doRemove">
            <Loader2 v-if="remove.isPending.value" class="animate-spin" /> Remove agent
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
