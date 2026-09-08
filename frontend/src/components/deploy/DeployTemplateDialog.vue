<script setup lang="ts">
// The deploy wizard: Target → Variables → Preflight → Deploy. Four steps because each answers a
// different question — where, with what, will it work there, and how — and the third one is
// the whole point of R5.6: render and validate against the host's facts BEFORE anything ships.
// Also the upgrade path: with `stackId` set, the same wizard applies a version to an existing
// stack, and Preflight shows the diff against what the host runs now.
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { ArrowLeft, ArrowRight, Loader2, Rocket, Check, TriangleAlert, ArrowUpCircle, Minus, X } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { SelectMenu } from '@/components/ui/select-menu'
import { Spinner } from '@/components/ui/spinner'
import { Badge } from '@/components/ui/badge'
import VariablesForm from './VariablesForm.vue'
import PreflightList from './PreflightList.vue'
import DiffView from './DiffView.vue'
import ComposeWarnings from './ComposeWarnings.vue'
import DeployOptionsForm from './DeployOptionsForm.vue'
import GatedButton from './GatedButton.vue'
import TemplateIcon from './TemplateIcon.vue'
import { useAgents, useTemplate, useTemplateVersion, useDryRun, useDeployTemplate, useStacks } from '@/lib/deploy/deployQueries'
import { defaultVariables, validateVariables, requirementChecks } from '@/lib/deploy/templates'
import { AGENT_STATUS_META } from '@/lib/deploy/agentStatus'
import { cn } from '@/lib/core/utils'
import type { DeployOptions, DryRunResult, Stack } from '@/lib/core/api'

const props = defineProps<{
  open: boolean
  templateId: string | null
  /** Pin a version; otherwise the template's latest. */
  versionId?: string | null
  /** Upgrade this stack in place (its host, name and current variables are seeded). */
  stack?: Stack | null
  prefillHost?: string | null
}>()
const emit = defineEmits<{ 'update:open': [boolean] }>()
const router = useRouter()

const templateQ = useTemplate(() => props.templateId ?? '')
const template = computed(() => templateQ.data.value ?? null)
const versionId = ref<string | null>(null)
const versionQ = useTemplateVersion(versionId)
const version = computed(() => versionQ.data.value ?? null)
const manifest = computed(() => version.value?.manifest ?? null)

const agentsQ = useAgents()
const hostOptions = computed(() => (agentsQ.data.value?.hosts ?? []).map((h) => ({ value: h.host, label: `${h.host}${h.site ? ` · ${h.site}` : ''}${!h.capabilities.includes('apply') ? ' (no apply)' : h.status !== 'online' ? ` (${AGENT_STATUS_META[h.status].label.toLowerCase()})` : ''}` })))
const agentFor = (host: string) => (agentsQ.data.value?.hosts ?? []).find((h) => h.host === host) ?? null

type Step = 'target' | 'variables' | 'preflight' | 'deploy'
const STEPS: { key: Step; label: string }[] = [{ key: 'target', label: 'Target' }, { key: 'variables', label: 'Variables' }, { key: 'preflight', label: 'Preflight' }, { key: 'deploy', label: 'Deploy' }]
const step = ref<Step>('target')
const stepIndex = computed(() => STEPS.findIndex((s) => s.key === step.value))

const host = ref('')
const name = ref('')
const values = ref<Record<string, string>>({})
const secrets = ref<Record<string, string>>({})
const options = ref<DeployOptions>({ pull: 'missing', force_recreate: false, recreate_volumes: false })
const acknowledged = ref<string[]>([])
const dry = ref<DryRunResult | null>(null)
const error = ref('')
// Errors for empty required fields appear only after an attempt to continue — a form that opens
// covered in red before anyone has typed reads as broken, not as guidance.
const attempted = ref(false)

watch(() => props.open, (o) => {
  if (!o) return
  step.value = props.stack ? 'variables' : 'target'
  versionId.value = props.versionId ?? null
  host.value = props.stack?.host ?? props.prefillHost ?? ''
  name.value = props.stack?.name ?? props.templateId ?? ''
  values.value = {}
  secrets.value = {}
  acknowledged.value = [...(props.stack?.acknowledged_warnings ?? [])]
  options.value = props.stack ? { ...props.stack.deploy_options } : { pull: 'missing', force_recreate: false, recreate_volumes: false }
  dry.value = null
  error.value = ''
  attempted.value = false
  // Reopening for the same template: the cached manifest is already here and its watcher will
  // not fire again, so seed the form now as well as when a manifest arrives.
  if (manifest.value) values.value = defaultVariables(manifest.value, props.stack?.variables ?? {})
}, { immediate: true })
// Latest version by default; seed the form the moment the manifest arrives.
watch([template, () => props.open], ([t, o]) => { if (t && o && !t.versions.some((v) => v.id === versionId.value)) versionId.value = t.versions[0]?.id ?? null }, { immediate: true })
watch(manifest, (m) => { if (m && props.open) values.value = defaultVariables(m, props.stack?.variables ?? {}) }, { immediate: true })

// A stack with this name already on this host means the deploy is an UPGRADE of it — said on the
// Target step, seeded from its variables, diffed against what it runs — never a silent replace.
const stacksQ = useStacks()
const existing = computed(() => props.stack ?? (stacksQ.data.value?.stacks ?? []).find((s) => s.host === host.value && s.name === name.value.trim() && s.status !== 'gone') ?? null)
watch(existing, (s, prev) => {
  if (s && s.id !== prev?.id && manifest.value) values.value = defaultVariables(manifest.value, s.variables)
})
const hostAgent = computed(() => (host.value ? agentFor(host.value) : null))
const hostBlocked = computed(() => (hostAgent.value && !hostAgent.value.capabilities.includes('apply') ? 'The agent on this host was installed without --docker-apply.' : null))
// The Target step's "what this template needs", marked against the chosen host's reported facts.
const checks = computed(() => (manifest.value ? requirementChecks(manifest.value, hostAgent.value?.docker ?? null) : []))
const hostHint = computed(() => (hostAgent.value && hostAgent.value.status !== 'online' ? `${hostAgent.value.host} is ${AGENT_STATUS_META[hostAgent.value.status].label.toLowerCase()} — the deploy waits as pending and applies on its next sync.` : 'A managed host whose agent can apply stacks.'))
const allErrors = computed(() => (manifest.value ? validateVariables(manifest.value, values.value, new Set([...Object.keys(secrets.value).filter((k) => secrets.value[k]), ...(props.stack?.secrets ?? [])])) : {}))
const errors = computed<Record<string, string>>(() => {
  if (attempted.value) return allErrors.value
  const shown: Record<string, string> = {}
  for (const [k, msg] of Object.entries(allErrors.value)) {
    const v = manifest.value?.variables.find((x) => x.name === k)
    const raw = v?.type === 'secret' ? secrets.value[k] : values.value[k]
    if (raw && raw.trim()) shown[k] = msg // a wrong value is always worth saying
  }
  return shown
})
const targetOk = computed(() => !!host.value && !hostBlocked.value && /^[a-z0-9][a-z0-9_-]*$/.test(name.value.trim()))
const variablesOk = computed(() => !!manifest.value && !Object.keys(allErrors.value).length)
const unacked = computed(() => (dry.value?.warnings ?? []).filter((w) => !acknowledged.value.includes(w.id)))
const preflightOk = computed(() => !!dry.value && dry.value.ok && !unacked.value.length)

const dryRun = useDryRun()
const deploy = useDeployTemplate()
function input() {
  return { template_version_id: versionId.value!, host: host.value, name: name.value.trim(), variables: values.value, secrets: secrets.value, deploy_options: options.value, acknowledged_warnings: acknowledged.value, stack_id: existing.value?.id ?? null }
}
async function next(): Promise<void> {
  error.value = ''
  if (step.value === 'target') step.value = 'variables'
  else if (step.value === 'variables') {
    if (!variablesOk.value) {
      attempted.value = true
      return
    }
    step.value = 'preflight'
    dry.value = null
    dry.value = await dryRun.mutateAsync(input())
    if (dry.value.error) error.value = dry.value.error
  } else if (step.value === 'preflight') step.value = 'deploy'
}
function back(): void {
  const i = stepIndex.value
  if (i > 0) step.value = STEPS[i - 1].key
}
async function submit(): Promise<void> {
  error.value = ''
  const res = await deploy.mutateAsync(input())
  if (res.ok && res.stack) {
    emit('update:open', false)
    router.push(`/deploy/stacks/${encodeURIComponent(res.stack.id)}`)
  } else error.value = res.error ?? 'Could not deploy.'
}
// Variables stays clickable when invalid so the click can reveal what is missing.
const canNext = computed(() => (step.value === 'target' ? targetOk.value : step.value === 'variables' ? targetOk.value : step.value === 'preflight' ? preflightOk.value : false))
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="flex max-h-[92vh] max-w-4xl flex-col gap-0 overflow-hidden p-0" data-testid="deploy-dialog">
      <!-- The steps scroll; the action row below stays put, so Back / Next never leave the screen on a long Variables form. -->
      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-6 [&>*]:min-w-0">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-3">
          <TemplateIcon v-if="template" :icon="template.icon" :name="template.name" size="sm" />
          <span>{{ existing ? `Upgrade ${existing.name} on ${existing.host}` : `Deploy ${template?.name ?? '…'}` }}</span>
          <Badge v-if="version" variant="brand" class="font-mono">{{ version.version }}</Badge>
        </DialogTitle>
        <DialogDescription>{{ stack ? `From ${stack.template_version ?? 'a hand-written compose'} to ${version?.version ?? '…'}. Preflight shows exactly what changes on the host before anything ships.` : 'Where, with what, will it work there, and how. Nothing leaves this Photon until the last step.' }}</DialogDescription>
      </DialogHeader>

      <!-- Stepper -->
      <ol class="flex items-center gap-2 text-xs" data-testid="deploy-steps">
        <li v-for="(s, i) in STEPS" :key="s.key" class="flex items-center gap-2" :data-step="s.key" :data-state="i < stepIndex ? 'done' : i === stepIndex ? 'current' : 'todo'">
          <span :class="cn('flex size-5 items-center justify-center rounded-full border text-[10px] font-semibold', i < stepIndex ? 'border-success bg-success text-white' : i === stepIndex ? 'border-brand bg-brand text-brand-foreground' : 'border-border text-muted-foreground')"><Check v-if="i < stepIndex" class="size-3" /><template v-else>{{ i + 1 }}</template></span>
          <span :class="i === stepIndex ? 'font-medium text-foreground' : 'text-muted-foreground'">{{ s.label }}</span>
          <span v-if="i < STEPS.length - 1" class="h-px w-6 bg-border" />
        </li>
      </ol>

      <div v-if="!template || !manifest" class="flex justify-center py-10"><Spinner size="md">Loading template…</Spinner></div>

      <!-- Target: a settings sheet — one choice per row, label left, control right — then the
           consequence of those choices (an upgrade banner) and the template's requirements marked
           against the host, so "will it work there" starts getting answered before Preflight. -->
      <div v-else-if="step === 'target'" class="flex flex-col" data-testid="step-target">
        <div class="grid gap-1.5 pb-3 sm:grid-cols-[minmax(0,220px)_minmax(0,1fr)] sm:gap-x-7">
          <div class="flex flex-col gap-0.5 sm:pt-2">
            <Label for="deploy-host" class="text-[13.5px] leading-snug">Host</Label>
            <p class="text-xs leading-relaxed text-muted-foreground">{{ hostHint }}</p>
          </div>
          <div class="flex flex-col gap-1.5">
            <div class="max-w-md"><SelectMenu id="deploy-host" :model-value="host" :options="[{ value: '', label: 'Choose a host…' }, ...hostOptions]" content-class="w-72" size="md" aria-label="Deploy host" @update:model-value="host = String($event)" /></div>
            <p v-if="hostBlocked" class="font-mono text-xs text-sev-error">{{ hostBlocked }}</p>
          </div>
        </div>
        <div class="grid gap-1.5 border-t border-border py-3 sm:grid-cols-[minmax(0,220px)_minmax(0,1fr)] sm:gap-x-7">
          <div class="flex flex-col gap-0.5 sm:pt-2">
            <Label for="deploy-version" class="text-[13.5px] leading-snug">Version</Label>
            <p class="text-xs leading-relaxed text-muted-foreground">Immutable snapshots. Pick an older one to reproduce what a customer runs.</p>
          </div>
          <div class="max-w-[14rem]"><SelectMenu id="deploy-version" :model-value="versionId ?? ''" :options="template.versions.map((v) => ({ value: v.id, label: `${v.version}${v.deployed_count ? ` · on ${v.deployed_count}` : ''}` }))" content-class="w-56" size="md" aria-label="Version" @update:model-value="versionId = String($event)" /></div>
        </div>
        <div class="grid gap-1.5 border-t border-border py-3 sm:grid-cols-[minmax(0,220px)_minmax(0,1fr)] sm:gap-x-7">
          <div class="flex flex-col gap-0.5 sm:pt-2">
            <Label for="deploy-name" class="text-[13.5px] leading-snug">Stack name</Label>
            <p class="text-xs leading-relaxed text-muted-foreground">Becomes the compose project name on the host.</p>
          </div>
          <div class="max-w-[14rem]"><Input id="deploy-name" v-model="name" class="font-mono" data-testid="deploy-name" autocomplete="off" /></div>
        </div>

        <div v-if="existing && !stack" class="mt-2 grid grid-cols-[auto_1fr] items-start gap-3 rounded-xl border border-sev-warn/30 bg-sev-warn-soft px-3.5 py-3 text-xs leading-relaxed text-sev-warn" data-testid="deploy-upgrade-note">
          <ArrowUpCircle class="mt-0.5 size-4" />
          <p><span class="block font-semibold">This is an upgrade, not a new stack</span><b>{{ existing.name }}</b> already runs on {{ existing.host }} ({{ existing.template_version ?? 'hand-written compose' }}, generation {{ existing.applied_generation ?? existing.desired_generation }}). Its variables are pre-filled and Preflight diffs against what it runs. Use another name for a second copy.</p>
        </div>

        <div v-if="checks.length" class="mt-3 rounded-xl border border-border px-3.5 pt-2.5 pb-1" data-testid="deploy-requirements">
          <div class="flex items-baseline justify-between gap-3">
            <p class="text-xs font-semibold text-foreground">What this template needs</p>
            <p class="text-xs text-muted-foreground">{{ hostAgent?.docker ? `against ${hostAgent.host}` : host ? 'host facts not reported yet' : 'choose a host to check' }}</p>
          </div>
          <ul class="mt-1 grid sm:grid-cols-2 sm:gap-x-7">
            <li v-for="(c, i) in checks" :key="c.key" :class="cn('grid grid-cols-[1rem_1fr_auto] items-center gap-2.5 border-t border-border py-1.5 text-xs', i === 0 && 'border-t-0', i === 1 && 'sm:border-t-0')" :data-state="c.state">
              <span :class="cn('flex size-4 items-center justify-center rounded-full', c.state === 'ok' ? 'bg-success-soft text-success' : c.state === 'fail' ? 'bg-sev-error-soft text-sev-error' : 'bg-muted text-muted-foreground')">
                <Check v-if="c.state === 'ok'" class="size-2.5" /><X v-else-if="c.state === 'fail'" class="size-2.5" /><Minus v-else class="size-2.5" />
              </span>
              <span class="text-foreground">{{ c.need }}</span>
              <span :class="cn('text-right', c.state === 'unknown' ? 'text-muted-foreground' : 'font-mono text-[11px] text-muted-foreground')">{{ c.have }}</span>
            </li>
          </ul>
        </div>
      </div>

      <!-- Variables -->
      <div v-else-if="step === 'variables'" data-testid="step-variables">
        <VariablesForm :manifest="manifest" :values="values" :secrets="secrets" :errors="errors" :stored-secrets="stack?.secrets ?? []" @update:values="values = $event" @update:secrets="secrets = $event" />
      </div>

      <!-- Preflight -->
      <div v-else-if="step === 'preflight'" class="flex flex-col gap-4" data-testid="step-preflight">
        <div v-if="dryRun.isPending.value || !dry" class="flex justify-center py-8"><Spinner size="md">Rendering and checking {{ host }}…</Spinner></div>
        <template v-else>
          <div v-if="dry.error" class="rounded-lg border border-sev-error/30 bg-sev-error-soft px-3 py-2 text-sm text-sev-error">{{ dry.error }}</div>
          <PreflightList :checks="dry.checks" />
          <div v-if="dry.generated.length || dry.secrets.length" class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
            <span v-if="dry.generated.length">Minted at deploy: <span class="font-mono text-foreground">{{ dry.generated.join(', ') }}</span></span>
            <span v-if="dry.secrets.length">Resolved on the agent: <span class="font-mono text-foreground">{{ dry.secrets.join(', ') }}</span></span>
          </div>
          <ComposeWarnings :warnings="dry.warnings" :acknowledged="acknowledged" @update:acknowledged="acknowledged = $event" />
          <DiffView :before="dry.previous_compose ?? ''" :after="dry.compose" :title="dry.previous_compose ? `compose.yaml — what changes on ${host}` : `compose.yaml — rendered for ${host}`" />
          <details class="rounded-xl border border-border bg-card shadow-1">
            <summary class="cursor-pointer px-3 py-2 text-xs font-medium text-foreground">Rendered .env</summary>
            <pre class="border-t border-border px-3 py-2 font-mono text-[11.5px] leading-5 text-foreground">{{ dry.env }}</pre>
          </details>
        </template>
      </div>

      <!-- Deploy -->
      <div v-else class="flex flex-col gap-4" data-testid="step-deploy">
        <DeployOptionsForm v-model="options" />
        <div class="rounded-xl border border-border bg-muted/40 p-3 text-sm">
          <p class="font-medium text-foreground"><Rocket class="mr-1 inline size-4 text-brand" /> {{ existing ? 'Upgrade' : 'Deploy' }} <span class="font-mono">{{ name }}</span> ({{ template.name }} {{ version?.version }}) on <span class="font-mono">{{ host }}</span></p>
          <p class="mt-1 text-xs text-muted-foreground">The rendered files are stored as a new generation and the host applies them on its next sync. A release marker lands on the service graphs once the first spans arrive.<span v-if="hostAgent && hostAgent.status !== 'online'"> The host is {{ AGENT_STATUS_META[hostAgent.status].label.toLowerCase() }}, so this waits as pending.</span></p>
        </div>
      </div>

      <p v-if="error" class="flex items-center gap-1.5 font-mono text-xs text-sev-error"><TriangleAlert class="size-3.5" /> {{ error }}</p>
      </div>

      <div class="flex shrink-0 items-center justify-between gap-2 border-t border-border bg-surface-2 px-6 py-3" data-testid="deploy-actions">
        <Button variant="ghost" :disabled="stepIndex === 0 || (!!stack && step === 'variables')" @click="back"><ArrowLeft class="size-4" /> Back</Button>
        <div class="flex items-center gap-2">
          <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
          <Button v-if="step !== 'deploy'" :disabled="!canNext || dryRun.isPending.value" data-testid="deploy-next" @click="next">{{ step === 'variables' ? 'Run preflight' : 'Next' }} <ArrowRight class="size-4" /></Button>
          <GatedButton v-else perm="deploy" variant="brand" :disabled="deploy.isPending.value || !preflightOk" data-testid="deploy-submit" @click="submit"><Loader2 v-if="deploy.isPending.value" class="animate-spin" /><Rocket v-else class="size-4" /> {{ existing ? 'Upgrade' : 'Deploy' }}</GatedButton>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
