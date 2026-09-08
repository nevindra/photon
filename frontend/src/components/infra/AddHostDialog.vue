<script setup lang="ts">
// "Add a host": enroll a machine under management. Two paths in one dialog.
//
// **Install over SSH** (the default) is the one-flow install from R1.9: addresses + SSH
// credentials in, an enrolled and synced host out. The master detects the OS/arch, pushes the
// binary it serves itself, writes a config with a freshly minted token, starts the unit and waits
// for the first sync. The operator pastes nothing and never sees the token. Credentials are used
// once and discarded unless "keep" is switched on — and that switch says what keeping them means.
// Several addresses at once run as one job with a stepper per host (bulk enrolment).
//
// **I'll install it myself** mints a token and hands over the install snippet for a host the
// master cannot SSH into (a customer box behind NAT, a laptop). The token is shown once.
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Check, X, Loader2, Circle, MinusCircle, ExternalLink, ChevronRight, TerminalSquare, KeyRound } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { FormField } from '@/components/ui/form-field'
import { NumberField } from '@/components/ui/number-field'
import { Switch } from '@/components/ui/switch'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { CAPABILITY_META } from '@/lib/deploy/agentStatus'
import { cn } from '@/lib/core/utils'
import { SelectMenu } from '@/components/ui/select-menu'
import { StatusPill } from '@/components/ui/status-pill'
import CapabilityPicker from '@/components/infra/CapabilityPicker.vue'
import AgentTokenPanel from '@/components/infra/AgentTokenPanel.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { useTenants } from '@/lib/tenants/tenantsQueries'
import { useEnrollAgent, useStartBootstrap, useBootstrapJob } from '@/lib/deploy/deployQueries'
import type { AgentCapability, BootstrapStep, SshBootstrapInput } from '@/lib/core/api'

const props = defineProps<{
  open: boolean
  /** Pre-fill the host name (the Agent tab's "Enroll this host"). Also selects the manual path. */
  prefillHost?: string | null
}>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const router = useRouter()
const tenantsQ = useTenants()
const siteOptions = computed(() => [{ value: '', label: 'This Photon (no Site)' }, ...(tenantsQ.data.value?.tenants ?? []).map((t) => ({ value: t.name, label: t.name }))])
const hasSites = computed(() => (tenantsQ.data.value?.tenants?.length ?? 0) > 0)

type Mode = 'ssh' | 'manual'
const mode = ref<Mode>('ssh')

// Shared fields
const site = ref('')
const capabilities = ref<AgentCapability[]>(['read', 'lifecycle'])
const pollSecs = ref(15)

// SSH fields
const addresses = ref('')
const sshUser = ref('root')
const port = ref(22)
const authType = ref<'password' | 'key'>('password')
const password = ref('')
const privateKey = ref('')
const passphrase = ref('')
const hostName = ref('')
const remember = ref(false)
const formError = ref('')

// Manual fields
const manualHost = ref('')
const minted = ref<{ token: string; host: string } | null>(null)

const enroll = useEnrollAgent()
const start = useStartBootstrap()
const jobId = ref<string | null>(null)
const jobQ = useBootstrapJob(jobId)
const job = computed(() => jobQ.data.value ?? null)

const addressList = computed(() => addresses.value.split(/\r?\n|,/).map((a) => a.trim()).filter(Boolean))
const multi = computed(() => addressList.value.length > 1)

watch(
  () => props.open,
  (o) => {
    if (o) {
      mode.value = props.prefillHost ? 'manual' : 'ssh'
      manualHost.value = props.prefillHost ?? ''
      hostName.value = ''
      addresses.value = ''
      password.value = ''
      privateKey.value = ''
      passphrase.value = ''
      remember.value = false
      formError.value = ''
      minted.value = null
      jobId.value = null
    } else {
      // Never let a secret or a finished stepper re-render on the next open.
      minted.value = null
      jobId.value = null
      password.value = ''
      privateKey.value = ''
    }
  },
  { immediate: true },
)

function sshInput(): SshBootstrapInput {
  return {
    addresses: addressList.value,
    port: port.value,
    username: sshUser.value.trim(),
    auth: authType.value === 'password' ? { type: 'password', password: password.value } : { type: 'key', private_key: privateKey.value, passphrase: passphrase.value || undefined },
    host_name: multi.value ? null : hostName.value.trim() || null,
    site: site.value || null,
    capabilities: capabilities.value,
    poll_secs: pollSecs.value,
    remember_credentials: remember.value,
  }
}

function validateSsh(): string {
  if (!addressList.value.length) return 'Enter at least one address.'
  if (!sshUser.value.trim()) return 'Enter the SSH user.'
  if (authType.value === 'password' && !password.value) return 'Enter the SSH password.'
  if (authType.value === 'key' && !privateKey.value.trim()) return 'Paste the private key.'
  if (pollSecs.value < 15) return 'Poll interval must be at least 15 seconds.'
  return ''
}

async function install(): Promise<void> {
  formError.value = validateSsh()
  if (formError.value) return
  const res = await start.mutateAsync(sshInput())
  if (res.ok && res.job) {
    jobId.value = res.job.id
    // The credentials have done their job; drop them from memory the moment the job starts.
    if (!remember.value) {
      password.value = ''
      privateKey.value = ''
      passphrase.value = ''
    }
  } else formError.value = res.error ?? 'Could not start the install.'
}

async function mint(): Promise<void> {
  formError.value = ''
  const name = manualHost.value.trim()
  if (!name) {
    formError.value = 'Enter the host name the agent will report as.'
    return
  }
  if (pollSecs.value < 15) {
    formError.value = 'Poll interval must be at least 15 seconds.'
    return
  }
  const res = await enroll.mutateAsync({ host: name, site: site.value || null, capabilities: capabilities.value, poll_secs: pollSecs.value })
  if (res.ok && res.token) minted.value = { token: res.token, host: name }
  else formError.value = res.error ?? 'Could not enroll the host.'
}

function retryFailed(): void {
  if (!job.value) return
  addresses.value = job.value.hosts.filter((h) => h.status === 'failed').map((h) => h.address).join('\n')
  jobId.value = null
}

function openHost(host: string): void {
  emit('update:open', false)
  router.push(`/infra/${encodeURIComponent(host)}/agent`)
}

const STEP_ICON: Record<BootstrapStep['status'], { icon: unknown; cls: string }> = {
  pending: { icon: Circle, cls: 'text-muted-foreground/40' },
  running: { icon: Loader2, cls: 'animate-spin text-brand' },
  done: { icon: Check, cls: 'text-success' },
  failed: { icon: X, cls: 'text-sev-error' },
  skipped: { icon: MinusCircle, cls: 'text-muted-foreground/40' },
}
const busy = computed(() => start.isPending.value || enroll.isPending.value)
// The folded "Agent settings" row says what it holds, so the defaults are read before they are
// accepted: "read + lifecycle · every 15 s · credentials discarded after install".
const agentSummary = computed(() => {
  const caps = capabilities.value.map((c) => CAPABILITY_META[c].label.toLowerCase()).join(' + ')
  const parts = [caps || 'read', `every ${pollSecs.value} s`]
  if (mode.value === 'ssh') parts.push(remember.value ? 'SSH credentials kept' : 'credentials discarded after install')
  return parts.join(' · ')
})
const MODES: { key: Mode; title: string; blurb: string; icon: unknown }[] = [
  { key: 'ssh', title: 'Install over SSH', blurb: 'Photon connects, pushes the agent, starts it and waits for the first sync. You paste nothing. Several addresses enroll at once.', icon: TerminalSquare },
  { key: 'manual', title: "I'll install it myself", blurb: 'For a box Photon cannot reach — a customer host behind NAT, a laptop. Photon mints a token and shows the install command once.', icon: KeyRound },
]
const doneHosts = computed(() => job.value?.hosts.filter((h) => h.status === 'done') ?? [])
const failedHosts = computed(() => job.value?.hosts.filter((h) => h.status === 'failed') ?? [])
// The headline says how far it got, in numbers when there are several hosts: "1 of 2 installed"
// is what an operator scans for, not a verdict on the batch.
const jobTitle = computed(() => {
  const j = job.value
  if (!j) return ''
  if (j.status === 'running') return 'Installing the agent…'
  const n = j.hosts.length
  if (n === 1) return j.status === 'done' ? 'Agent installed' : 'Install did not finish'
  return `${doneHosts.value.length} of ${n} hosts installed`
})
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="flex max-h-[88vh] max-w-2xl flex-col gap-0 overflow-hidden p-0" data-testid="add-host-dialog">
      <!-- The form scrolls; the action row below stays put. -->
      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-6 [&>*]:min-w-0">
      <DialogHeader>
        <DialogTitle>{{ job ? jobTitle : minted ? 'Host enrolled' : 'Add a host' }}</DialogTitle>
        <DialogDescription>
          <template v-if="job">{{ job.hosts.length === 1 ? 'One host' : `${job.hosts.length} hosts` }} over SSH{{ job.hosts.some((h) => h.address.includes('@')) ? '' : ` as ${sshUser}` }}. {{ job.credentials_kept ? 'SSH credentials were kept for reinstalls.' : 'SSH credentials were used once and discarded.' }}</template>
          <template v-else-if="minted">Install the agent on the host with this token. It appears under Hosts once its first sync lands.</template>
          <template v-else>Put a machine under Docker management. Metrics need nothing more than the agent; Docker needs the capabilities you pick here.</template>
        </DialogDescription>
      </DialogHeader>

      <!-- Bootstrap progress -->
      <div v-if="job" class="flex flex-col gap-3" data-testid="bootstrap-progress">
        <div v-for="h in job.hosts" :key="h.address" class="rounded-xl border border-border bg-card p-3">
          <div class="flex items-center justify-between gap-2">
            <span class="font-mono text-sm text-foreground">{{ h.host ?? h.address }}<span v-if="h.host && h.host !== h.address" class="ml-1.5 text-xs text-muted-foreground">{{ h.address }}</span></span>
            <StatusPill :tone="h.status === 'done' ? 'success' : h.status === 'failed' ? 'error' : 'info'" :dot="h.status !== 'running'">
              <Loader2 v-if="h.status === 'running'" class="animate-spin" />
              {{ h.status === 'done' ? 'Enrolled and synced' : h.status === 'failed' ? 'Failed' : 'Installing' }}
            </StatusPill>
          </div>
          <ol class="mt-2 flex flex-col gap-1">
            <li v-for="s in h.steps" :key="s.key" class="flex items-start gap-2 text-xs" :data-step="s.key" :data-status="s.status">
              <component :is="STEP_ICON[s.status].icon" :class="['mt-0.5 size-3.5 shrink-0', STEP_ICON[s.status].cls]" />
              <span :class="s.status === 'pending' || s.status === 'skipped' ? 'text-muted-foreground/60' : 'text-foreground'">{{ s.label }}</span>
              <span v-if="s.detail" :class="['ml-auto text-right font-mono', s.status === 'failed' ? 'text-sev-error' : 'text-muted-foreground']">{{ s.detail }}</span>
            </li>
          </ol>
        </div>
        <div class="flex items-center justify-end gap-2">
          <Button v-if="failedHosts.length && job.status !== 'running'" variant="outline" data-testid="bootstrap-retry" @click="retryFailed">Retry {{ failedHosts.length === 1 ? 'failed host' : `${failedHosts.length} failed hosts` }}</Button>
          <Button v-if="job.status !== 'running'" variant="ghost" @click="jobId = null; addresses = ''">Add another</Button>
          <Button v-if="doneHosts.length === 1 && doneHosts[0].host" data-testid="bootstrap-open-host" @click="openHost(doneHosts[0].host!)"><ExternalLink class="size-4" /> Open {{ doneHosts[0].host }}</Button>
          <Button v-else-if="job.status !== 'running'" @click="emit('update:open', false)">Done</Button>
        </div>
      </div>

      <!-- Manual: minted token -->
      <div v-else-if="minted" class="flex flex-col gap-3">
        <AgentTokenPanel :token="minted.token" :host="minted.host" :capabilities="capabilities" :poll-secs="pollSecs" />
        <div class="flex justify-end gap-2">
          <Button variant="ghost" @click="minted = null; manualHost = ''">Add another</Button>
          <Button data-testid="manual-open-host" @click="openHost(minted.host)"><ExternalLink class="size-4" /> Open {{ minted.host }}</Button>
        </div>
      </div>

      <!-- Form: first the one choice that decides everything else (two radio cards), then only
           that path's fields, then the agent settings that rarely change folded under a line
           that says what they are. -->
      <form v-else id="add-host-form" class="flex flex-col gap-4" data-testid="add-host-form" @submit.prevent="mode === 'ssh' ? install() : mint()">
        <div class="grid gap-3 sm:grid-cols-2" role="radiogroup" aria-label="How to install the agent" data-testid="add-host-mode">
          <button
            v-for="m in MODES"
            :key="m.key"
            type="button"
            role="radio"
            :aria-checked="mode === m.key"
            :data-mode="m.key"
            :class="cn('grid grid-cols-[1rem_1fr] items-start gap-3 rounded-xl border p-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring', mode === m.key ? 'border-brand bg-brand-soft' : 'border-border hover:border-foreground/20')"
            @click="mode = m.key"
          >
            <span :class="cn('mt-0.5 size-4 rounded-full border', mode === m.key ? 'border-[5px] border-brand' : 'border-border-strong')" />
            <span>
              <span class="flex items-center gap-1.5 text-sm font-semibold text-foreground"><component :is="m.icon" class="size-3.5 text-muted-foreground" /> {{ m.title }}</span>
              <span class="mt-0.5 block text-xs leading-relaxed text-muted-foreground">{{ m.blurb }}</span>
            </span>
          </button>
        </div>

        <template v-if="mode === 'ssh'">
          <FormField label="Address" for="ssh-addresses" :hint="multi ? `${addressList.length} hosts — each is installed in parallel and named after its address.` : 'host, user@host or host:port. Several lines enroll several hosts at once.'">
            <Textarea id="ssh-addresses" v-model="addresses" mono data-testid="ssh-addresses" placeholder="10.20.0.14&#10;ops@edge-03.plant.local" class="min-h-[3.25rem]" />
          </FormField>
          <div class="grid grid-cols-[1fr_7rem] gap-3">
            <FormField label="SSH user" for="ssh-user" hint="root, or a user with sudo — installing a service needs it. A user@ prefix on an address wins.">
              <Input id="ssh-user" v-model="sshUser" data-testid="ssh-user" autocomplete="off" />
            </FormField>
            <FormField label="Port" for="ssh-port">
              <NumberField id="ssh-port" v-model="port" :min="1" :max="65535" />
            </FormField>
          </div>
          <FormField label="Authentication" for="ssh-auth">
            <Segmented id="ssh-auth" v-model="authType" class="mb-2" data-testid="ssh-auth">
              <SegmentedItem value="password">Password</SegmentedItem>
              <SegmentedItem value="key">Private key</SegmentedItem>
            </Segmented>
            <Input v-if="authType === 'password'" v-model="password" type="password" data-testid="ssh-password" autocomplete="off" placeholder="SSH password — used once, then discarded" />
            <div v-else class="flex flex-col gap-2">
              <Textarea v-model="privateKey" mono data-testid="ssh-key" placeholder="-----BEGIN OPENSSH PRIVATE KEY-----" class="min-h-[5rem]" />
              <Input v-model="passphrase" type="password" autocomplete="off" placeholder="Key passphrase (optional)" />
            </div>
          </FormField>
          <div class="grid gap-3" :class="hasSites ? 'sm:grid-cols-2' : ''">
            <FormField label="Host name" for="ssh-host-name" optional :hint="multi ? 'Several addresses: each host is named after its own hostname.' : 'Defaults to the hostname the agent detects.'">
              <Input id="ssh-host-name" v-model="hostName" data-testid="ssh-host-name" autocomplete="off" placeholder="edge-03" :disabled="multi" />
            </FormField>
            <FormField v-if="hasSites" label="Site" for="host-site" hint="Which Photon this host belongs to. Central can only install where it can reach.">
              <SelectMenu id="host-site" :model-value="site" :options="siteOptions" content-class="w-56" size="md" @update:model-value="site = String($event)" />
            </FormField>
          </div>
        </template>
        <template v-else>
          <div class="grid gap-3" :class="hasSites ? 'sm:grid-cols-2' : ''">
            <FormField label="Host name" for="manual-host" hint="What the agent reports as host.name — the same name /infra shows.">
              <Input id="manual-host" v-model="manualHost" data-testid="manual-host" autocomplete="off" placeholder="edge-03" />
            </FormField>
            <FormField v-if="hasSites" label="Site" for="host-site-manual">
              <SelectMenu id="host-site-manual" :model-value="site" :options="siteOptions" content-class="w-56" size="md" @update:model-value="site = String($event)" />
            </FormField>
          </div>
        </template>

        <details class="group rounded-xl border border-border" data-testid="agent-settings">
          <summary class="flex cursor-pointer list-none items-center gap-2.5 px-3.5 py-2.5 text-sm [&::-webkit-details-marker]:hidden">
            <ChevronRight class="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
            <span class="font-medium text-foreground">Agent settings</span>
            <span class="ml-auto truncate text-xs text-muted-foreground" data-testid="agent-settings-summary">{{ agentSummary }}</span>
          </summary>
          <div class="flex flex-col gap-4 border-t border-border bg-muted/40 p-3.5">
            <FormField label="Capabilities" hint="What the agent will be allowed to do on this host. Opt-in on the host side: Photon cannot widen them later without a reinstall.">
              <CapabilityPicker v-model="capabilities" />
            </FormField>
            <div class="grid gap-3 sm:grid-cols-2">
              <FormField label="Poll interval" for="host-poll" hint="How often the agent asks for work. 15s is the floor; slower is fine for a quiet site.">
                <NumberField id="host-poll" v-model="pollSecs" :min="15" :max="3600" unit="s" />
              </FormField>
              <div v-if="mode === 'ssh'" class="flex flex-col gap-1.5">
                <label class="flex items-center justify-between gap-3 text-sm font-medium text-foreground" for="ssh-remember">
                  Keep SSH credentials
                  <Switch id="ssh-remember" :model-value="remember" data-testid="ssh-remember" @update:model-value="remember = $event" />
                </label>
                <p class="text-xs text-muted-foreground">
                  {{ remember ? 'Kept, encrypted, for reinstalls and upgrades that cannot ride the sync channel. A master that keeps shell keys to every host is a master whose compromise is the fleet’s.' : 'Off: used once for this install, then discarded. Upgrades ride the agent’s own sync channel, so nothing needs them again.' }}
                </p>
              </div>
            </div>
          </div>
        </details>
        <p v-if="formError" class="font-mono text-xs text-sev-error" data-testid="add-host-error">{{ formError }}</p>
      </form>
      </div>

      <!-- The action row: outside the scroll, so Install is always in reach. -->
      <div v-if="!job && !minted" class="flex shrink-0 items-center justify-between gap-2 border-t border-border bg-surface-2 px-6 py-3" data-testid="add-host-actions">
        <span class="text-xs text-muted-foreground">{{ mode === 'ssh' ? 'Nothing leaves this Photon until you press Install.' : 'The token is shown once, after minting.' }}</span>
        <div class="flex items-center gap-2">
          <Button type="button" variant="ghost" @click="emit('update:open', false)">Cancel</Button>
          <GatedButton perm="agents" type="submit" form="add-host-form" :disabled="busy" data-testid="add-host-submit">
            <Loader2 v-if="busy" class="animate-spin" />
            {{ mode === 'ssh' ? (busy ? 'Starting…' : multi ? `Install on ${addressList.length} hosts` : 'Install agent') : (busy ? 'Minting…' : 'Mint token') }}
          </GatedButton>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
