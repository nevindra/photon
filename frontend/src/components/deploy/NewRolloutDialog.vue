<script setup lang="ts">
// The rollout wizard: Template → Targets → Variables → Strategy → Review. Targets are a selector
// (hosts, a Site, or labels — federation's tenant model, not a second grouping, R6.1) resolved
// live so the operator sees who is in, who waits (offline), and who is out (no apply). Strategy is
// the canary size, the gate, the failure threshold and pre-pull. Review shows the stage plan.
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { ArrowLeft, ArrowRight, Check, Loader2, Rocket, Plus, X } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { SelectMenu } from '@/components/ui/select-menu'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Checkbox } from '@/components/ui/checkbox'
import { Switch } from '@/components/ui/switch'
import { NumberField } from '@/components/ui/number-field'
import { Badge } from '@/components/ui/badge'
import { Spinner } from '@/components/ui/spinner'
import VariablesForm from './VariablesForm.vue'
import DeployOptionsForm from './DeployOptionsForm.vue'
import ComposeWarnings from './ComposeWarnings.vue'
import { StatusPill } from '@/components/ui/status-pill'
import GatedButton from './GatedButton.vue'
import TemplateIcon from './TemplateIcon.vue'
import { useAgents, useTemplates, useTemplateVersion, useCreateRollout, useDryRun, useStacks } from '@/lib/deploy/deployQueries'
import { lintCompose } from '@/lib/deploy/compose'
import { PREFLIGHT_META } from '@/lib/deploy/templates'
import { useTenants } from '@/lib/tenants/tenantsQueries'
import { defaultVariables, validateVariables } from '@/lib/deploy/templates'
import { resolveTargets, planStages, canarySize, DEFAULT_STRATEGY, defaultRolloutName, strategySummary } from '@/lib/deploy/rollouts'
import { AGENT_STATUS_META } from '@/lib/deploy/agentStatus'
import { cn } from '@/lib/core/utils'
import type { DeployOptions, DryRunResult, RolloutStrategy, TargetSelector } from '@/lib/core/api'

const props = defineProps<{ open: boolean; templateId?: string | null; versionId?: string | null; prefillHosts?: string[] | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()
const router = useRouter()

type Step = 'template' | 'targets' | 'variables' | 'strategy' | 'review'
const STEPS: { key: Step; label: string }[] = [{ key: 'template', label: 'Template' }, { key: 'targets', label: 'Targets' }, { key: 'variables', label: 'Variables' }, { key: 'strategy', label: 'Strategy' }, { key: 'review', label: 'Review' }]
const step = ref<Step>('template')
const stepIndex = computed(() => STEPS.findIndex((s) => s.key === step.value))

const templatesQ = useTemplates()
const templates = computed(() => templatesQ.data.value?.templates ?? [])
const templateId = ref('')
const template = computed(() => templates.value.find((t) => t.id === templateId.value) ?? null)
const versionId = ref('')
const versionQ = useTemplateVersion(() => versionId.value || null)
const version = computed(() => versionQ.data.value ?? null)
const manifest = computed(() => version.value?.manifest ?? null)
watch([template, () => props.open], ([t, o]) => { if (t && o && !t.versions.some((v) => v.id === versionId.value)) versionId.value = t.versions[0]?.id ?? '' }, { immediate: true })

const agentsQ = useAgents()
const hosts = computed(() => agentsQ.data.value?.hosts ?? [])
const stacksQ = useStacks()
const existingStacks = computed(() => (stacksQ.data.value?.stacks ?? []).filter((s) => s.status !== 'gone'))
const tenantsQ = useTenants()
// Sites come from the tenant registry AND from what the managed hosts report, so a host enrolled
// under a Site name is targetable even before that Site is registered here.
const sites = computed(() => Array.from(new Set([...(tenantsQ.data.value?.tenants ?? []).map((t) => t.name), ...hosts.value.map((h) => h.site).filter((s): s is string => !!s)])).sort())

const kind = ref<TargetSelector['kind']>('hosts')
const pickedHosts = ref<string[]>([])
const pickedSites = ref<string[]>([])
const labels = ref<{ k: string; v: string }[]>([{ k: '', v: '' }])
const selector = computed<TargetSelector>(() => (kind.value === 'hosts' ? { kind: 'hosts', hosts: pickedHosts.value } : kind.value === 'site' ? { kind: 'site', sites: pickedSites.value } : { kind: 'labels', labels: Object.fromEntries(labels.value.filter((l) => l.k.trim()).map((l) => [l.k.trim(), l.v.trim()])) }))
const resolved = computed(() => resolveTargets(selector.value, hosts.value))
const eligible = computed(() => resolved.value.filter((t) => t.eligible))

const stackName = ref('')
const name = ref('')
const nameTouched = ref(false)
const values = ref<Record<string, string>>({})
const secrets = ref<Record<string, string>>({})
const attempted = ref(false)
const strategy = ref<RolloutStrategy>({ ...DEFAULT_STRATEGY })
const options = ref<DeployOptions>({ pull: 'always', force_recreate: false, recreate_volumes: false })
const acknowledged = ref<string[]>([])
const startNow = ref(true)
const error = ref('')
// Per-target preflight, run when the Review step opens: the same dry-run the single deploy gets,
// once per resolved host, so a full disk or a taken port is known before anything ships.
const preflight = ref<Record<string, DryRunResult>>({})
const preflightBusy = ref(false)
const dryRun = useDryRun()

watch(() => props.open, (o) => {
  if (!o) return
  step.value = 'template'
  templateId.value = props.templateId ?? ''
  versionId.value = props.versionId ?? ''
  kind.value = props.prefillHosts?.length ? 'hosts' : sites.value.length ? 'site' : 'hosts'
  pickedHosts.value = [...(props.prefillHosts ?? [])]
  pickedSites.value = []
  labels.value = [{ k: '', v: '' }]
  values.value = {}
  secrets.value = {}
  attempted.value = false
  strategy.value = { ...DEFAULT_STRATEGY }
  options.value = { pull: 'always', force_recreate: false, recreate_volumes: false }
  acknowledged.value = []
  preflight.value = {}
  startNow.value = true
  nameTouched.value = false
  name.value = ''
  error.value = ''
}, { immediate: true })
watch([template, () => props.open], ([t, o]) => { if (t && o) stackName.value = t.id }, { immediate: true })
watch(manifest, (m) => { if (m) values.value = defaultVariables(m) })
watch([template, version, selector], () => { if (!nameTouched.value && template.value && version.value) name.value = defaultRolloutName(template.value.name, version.value.version, selector.value) })

// A secret every target already holds (an upgrade of stacks deployed before) need not be typed
// again; a target without it still needs the value, so "stored" means stored EVERYWHERE.
const storedSecrets = computed(() => {
  if (!manifest.value || !eligible.value.length) return [] as string[]
  return manifest.value.variables.filter((v) => v.type === 'secret').map((v) => v.name).filter((n) => eligible.value.every((t) => existingStacks.value.find((s) => s.host === t.host && s.name === stackName.value)?.secrets.includes(n)))
})
const allErrors = computed(() => (manifest.value ? validateVariables(manifest.value, values.value, new Set([...Object.keys(secrets.value).filter((k) => secrets.value[k]), ...storedSecrets.value])) : {}))
const errors = computed(() => (attempted.value ? allErrors.value : {}))
// Once preflight has run, a blocked host is out of the plan — never in the canary slot.
const planInput = computed(() => resolved.value.map((t) => (preflight.value[t.host] && !preflight.value[t.host].ok ? { ...t, eligible: false, note: 'blocked by preflight' } : t)))
const plan = computed(() => planStages(planInput.value, strategy.value))
// Who is out and why — the operator should not have to go back a step and guess.
const skipped = computed(() => planInput.value.filter((t) => !t.eligible).map((t) => ({ host: t.host, why: preflight.value[t.host] && !preflight.value[t.host].ok ? preflightNote(t.host).replace(/^skipped: /, '') : (t.note ?? 'cannot apply') })))
const noCanary = computed(() => !preflightBusy.value && Object.keys(preflight.value).length > 0 && !plan.value.some((p) => p.stage === 1))
// Dangerous constructs in the version being rolled out (R8.4) — every one needs a tick, here, on
// the fleet path too, not only on the single-host deploy.
const warnings = computed(() => (version.value ? lintCompose(version.value.compose_text) : []))
const unacked = computed(() => warnings.value.filter((w) => !acknowledged.value.includes(w.id)))
const preflightFailed = computed(() => Object.values(preflight.value).filter((r) => !r.ok).length)
async function runPreflight(): Promise<void> {
  preflightBusy.value = true
  preflight.value = {}
  const results = await Promise.all(eligible.value.map(async (t) => [t.host, await dryRun.mutateAsync({ template_version_id: versionId.value, host: t.host, name: stackName.value, variables: values.value, secrets: secrets.value, deploy_options: options.value, acknowledged_warnings: acknowledged.value })] as const))
  preflight.value = Object.fromEntries(results)
  preflightBusy.value = false
}
function preflightTone(host: string): 'success' | 'error' | 'warning' | 'neutral' {
  const r = preflight.value[host]
  if (!r) return 'neutral'
  if (!r.ok) return 'error'
  return r.checks.some((c) => c.status === 'warn') ? 'warning' : 'success'
}
function preflightNote(host: string): string {
  const r = preflight.value[host]
  if (!r) return preflightBusy.value ? 'checking…' : 'not checked'
  const fail = r.checks.find((c) => c.status === 'fail')
  if (fail) return `skipped: ${fail.detail}`
  const warn = r.checks.find((c) => c.status === 'warn')
  return warn ? warn.detail : 'ready'
}
// Counts on the Review step follow the plan, which excludes preflight-blocked hosts — the summary
// must not promise 2 hosts when one is skipped.
const planned = computed(() => planInput.value.filter((t) => t.eligible))
const canaryN = computed(() => canarySize(planned.value.length, strategy.value))
const stages = computed(() => (planned.value.length > canaryN.value ? 2 : 1))

const canNext = computed(() => {
  if (step.value === 'template') return !!template.value && !!versionId.value
  if (step.value === 'targets') return eligible.value.length > 0 && /^[a-z0-9][a-z0-9_-]*$/.test(stackName.value)
  if (step.value === 'variables') return true
  if (step.value === 'strategy') return strategy.value.canary >= 1 && strategy.value.failure_threshold >= 1
  return false
})
function next(): void {
  if (step.value === 'variables' && Object.keys(allErrors.value).length) {
    attempted.value = true
    return
  }
  step.value = STEPS[stepIndex.value + 1].key
  if (step.value === 'review') void runPreflight()
}
function back(): void {
  if (stepIndex.value > 0) step.value = STEPS[stepIndex.value - 1].key
}
function toggleHost(h: string, on: boolean): void {
  pickedHosts.value = on ? [...new Set([...pickedHosts.value, h])] : pickedHosts.value.filter((x) => x !== h)
}
function toggleSite(s: string, on: boolean): void {
  pickedSites.value = on ? [...new Set([...pickedSites.value, s])] : pickedSites.value.filter((x) => x !== s)
}

const create = useCreateRollout()
async function submit(): Promise<void> {
  error.value = ''
  const res = await create.mutateAsync({ name: name.value, template_version_id: versionId.value, stack_name: stackName.value, selector: selector.value, strategy: strategy.value, variables: values.value, secrets: secrets.value, deploy_options: options.value, acknowledged_warnings: acknowledged.value, start: startNow.value })
  if (res.ok && res.rollout) {
    emit('update:open', false)
    router.push(`/deploy/rollouts/${encodeURIComponent(res.rollout.id)}`)
  } else error.value = res.error ?? 'Could not create the rollout.'
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="flex max-h-[92vh] max-w-4xl flex-col gap-0 overflow-hidden p-0" data-testid="rollout-dialog">
      <!-- The steps scroll; the action row below stays put, so Back / Next never leave the screen on a long Variables form. -->
      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-6 [&>*]:min-w-0">
      <DialogHeader>
        <DialogTitle>New rollout</DialogTitle>
        <DialogDescription>One template version to many hosts, staged: a canary first, an explicit gate, then the rest. Offline sites wait as pending; a stage pauses itself when too many fail.</DialogDescription>
      </DialogHeader>

      <ol class="flex flex-wrap items-center gap-2 text-xs" data-testid="rollout-steps">
        <li v-for="(s, i) in STEPS" :key="s.key" class="flex items-center gap-2" :data-step="s.key" :data-state="i < stepIndex ? 'done' : i === stepIndex ? 'current' : 'todo'">
          <span :class="cn('flex size-5 items-center justify-center rounded-full border text-[10px] font-semibold', i < stepIndex ? 'border-success bg-success text-white' : i === stepIndex ? 'border-brand bg-brand text-brand-foreground' : 'border-border text-muted-foreground')"><Check v-if="i < stepIndex" class="size-3" /><template v-else>{{ i + 1 }}</template></span>
          <span :class="i === stepIndex ? 'font-medium text-foreground' : 'text-muted-foreground'">{{ s.label }}</span>
          <span v-if="i < STEPS.length - 1" class="h-px w-5 bg-border" />
        </li>
      </ol>

      <!-- Template -->
      <div v-if="step === 'template'" class="flex flex-col gap-3" data-testid="step-template">
        <div v-if="!templates.length" class="flex justify-center py-8"><Spinner size="md">Loading templates…</Spinner></div>
        <div v-else class="grid gap-2 sm:grid-cols-2" role="radiogroup">
          <button v-for="t in templates" :key="t.id" type="button" role="radio" :aria-checked="templateId === t.id" :data-pick-template="t.id" :class="cn('flex items-start gap-3 rounded-xl border p-3 text-left transition-colors', templateId === t.id ? 'border-brand bg-brand-soft/40' : 'border-border bg-card hover:border-foreground/20')" @click="templateId = t.id">
            <TemplateIcon :icon="t.icon" :name="t.name" size="sm" />
            <span class="min-w-0"><span class="block text-sm font-medium text-foreground">{{ t.name }} <span class="font-mono text-xs text-muted-foreground">{{ t.latest_version }}</span></span><span class="line-clamp-1 text-xs text-muted-foreground">{{ t.description }}</span></span>
          </button>
        </div>
        <FormField v-if="template" label="Version" for="ro-version" hint="Immutable; rollback re-applies whatever each host ran before.">
          <SelectMenu id="ro-version" :model-value="versionId" :options="template.versions.map((v) => ({ value: v.id, label: `${v.version}${v.deployed_count ? ` · on ${v.deployed_count}` : ''}` }))" content-class="w-56" size="md" aria-label="Rollout version" @update:model-value="versionId = String($event)" />
        </FormField>
      </div>

      <!-- Targets -->
      <div v-else-if="step === 'targets'" class="grid gap-4 lg:grid-cols-[1fr_18rem]" data-testid="step-targets">
        <div class="flex flex-col gap-3">
          <Segmented v-model="kind" class="w-fit" data-testid="selector-kind">
            <SegmentedItem value="hosts">Hosts</SegmentedItem>
            <SegmentedItem value="site" :disabled="!sites.length" :title="sites.length ? undefined : 'No Sites registered on this Photon'">Site</SegmentedItem>
            <SegmentedItem value="labels">Labels</SegmentedItem>
          </Segmented>
          <div v-if="kind === 'hosts'" class="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface-1" data-testid="pick-hosts">
            <label v-for="h in hosts" :key="h.host" class="flex cursor-pointer items-center gap-3 px-3 py-2 text-sm" :data-host="h.host">
              <Checkbox :model-value="pickedHosts.includes(h.host)" @update:model-value="toggleHost(h.host, $event === true)" />
              <span class="font-mono text-foreground">{{ h.host }}</span>
              <Badge v-if="h.site" variant="outline">{{ h.site }}</Badge>
              <span class="ml-auto text-xs text-muted-foreground">{{ !h.capabilities.includes('apply') ? 'no apply' : AGENT_STATUS_META[h.status].label.toLowerCase() }}</span>
            </label>
          </div>
          <div v-else-if="kind === 'site'" class="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface-1" data-testid="pick-sites">
            <label v-for="s in sites" :key="s" class="flex cursor-pointer items-center gap-3 px-3 py-2 text-sm">
              <Checkbox :model-value="pickedSites.includes(s)" @update:model-value="toggleSite(s, $event === true)" />
              <span class="text-foreground">{{ s }}</span>
              <span class="ml-auto text-xs text-muted-foreground">{{ hosts.filter((h) => h.site === s).length }} managed {{ hosts.filter((h) => h.site === s).length === 1 ? 'host' : 'hosts' }}</span>
            </label>
            <label class="flex cursor-pointer items-center gap-3 px-3 py-2 text-sm">
              <Checkbox :model-value="pickedSites.includes('')" @update:model-value="toggleSite('', $event === true)" />
              <span class="text-foreground">This Photon (no Site)</span>
              <span class="ml-auto text-xs text-muted-foreground">{{ hosts.filter((h) => !h.site).length }} managed {{ hosts.filter((h) => !h.site).length === 1 ? 'host' : 'hosts' }}</span>
            </label>
          </div>
          <div v-else class="flex flex-col gap-2" data-testid="pick-labels">
            <div v-for="(l, i) in labels" :key="i" class="flex items-center gap-2">
              <Input v-model="l.k" placeholder="key" class="h-8 font-mono text-xs" />
              <span class="text-muted-foreground">=</span>
              <Input v-model="l.v" placeholder="value" class="h-8 font-mono text-xs" />
              <Button variant="ghost" size="icon" class="size-8 text-muted-foreground" :disabled="labels.length === 1" @click="labels.splice(i, 1)"><X class="size-4" /></Button>
            </div>
            <Button variant="outline" size="sm" class="w-fit" @click="labels.push({ k: '', v: '' })"><Plus class="size-4" /> Label</Button>
            <p class="text-xs text-muted-foreground">Every label must match. No labels = every managed host.</p>
          </div>
          <FormField label="Stack name on each host" for="ro-stack" hint="The compose project name; an existing stack with this name is upgraded in place.">
            <Input id="ro-stack" v-model="stackName" class="font-mono" data-testid="rollout-stack-name" />
          </FormField>
        </div>
        <aside class="rounded-xl border border-border bg-card p-3 shadow-1" data-testid="resolved-targets">
          <p class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Resolved targets</p>
          <p class="mt-1 text-sm text-foreground">{{ eligible.length }} of {{ resolved.length }} eligible <span class="text-xs font-normal text-muted-foreground">· each is preflighted at Review</span></p>
          <ul class="mt-2 flex flex-col gap-1 text-xs">
            <li v-for="t in resolved" :key="t.host" class="flex items-center gap-2" :data-resolved="t.host">
              <span :class="cn('size-1.5 rounded-full', !t.eligible ? 'bg-muted-foreground/40' : t.agent_status === 'online' ? 'bg-success' : 'bg-muted-foreground')" />
              <span class="font-mono" :class="t.eligible ? 'text-foreground' : 'text-muted-foreground line-through'">{{ t.host }}</span>
              <span v-if="t.note" class="truncate text-muted-foreground">{{ t.note }}</span>
            </li>
            <li v-if="!resolved.length" class="text-muted-foreground">Nothing selected yet.</li>
          </ul>
        </aside>
      </div>

      <!-- Variables -->
      <div v-else-if="step === 'variables'" data-testid="step-variables">
        <p class="mb-3 text-xs text-muted-foreground">The same values go to every target. Generated secrets are minted per host.</p>
        <VariablesForm v-if="manifest" :manifest="manifest" :values="values" :secrets="secrets" :errors="errors" :stored-secrets="storedSecrets" @update:values="values = $event" @update:secrets="secrets = $event" />
      </div>

      <!-- Strategy -->
      <div v-else-if="step === 'strategy'" class="grid gap-4 sm:grid-cols-2" data-testid="step-strategy">
        <FormField label="Canary" for="ro-canary" :hint="`Stage 1 gets ${canaryN} host${canaryN === 1 ? '' : 's'}; online hosts are chosen first so the gate can be judged.`">
          <div class="flex items-center gap-2">
            <NumberField id="ro-canary" v-model="strategy.canary" :min="1" :max="strategy.canary_is_percent ? 100 : 500" class="w-28" />
            <Segmented :model-value="strategy.canary_is_percent ? 'pct' : 'n'" size="md" @update:model-value="strategy.canary_is_percent = $event === 'pct'"><SegmentedItem value="n">hosts</SegmentedItem><SegmentedItem value="pct">%</SegmentedItem></Segmented>
          </div>
        </FormField>
        <FormField label="Pause the stage after" for="ro-threshold" hint="Failures in one stage. The rollout stops advancing until you retry, resume, or roll back.">
          <div class="flex items-center gap-2"><NumberField id="ro-threshold" v-model="strategy.failure_threshold" :min="1" class="w-28" /><span class="text-sm text-muted-foreground">failure{{ strategy.failure_threshold === 1 ? '' : 's' }}</span></div>
        </FormField>
        <label class="flex items-start justify-between gap-3 rounded-xl border border-border bg-card p-3" for="ro-prepull">
          <span class="text-sm"><span class="font-medium text-foreground">Pre-pull images</span><span class="block text-xs text-muted-foreground">Fetch before the restart window. On a slow customer link this is the difference between 30 seconds of downtime and 20 minutes.</span></span>
          <Switch id="ro-prepull" :model-value="strategy.prepull" @update:model-value="strategy.prepull = $event" />
        </label>
        <label class="flex items-start justify-between gap-3 rounded-xl border border-border bg-card p-3" for="ro-auto">
          <span class="text-sm"><span class="font-medium text-foreground">Auto-promote past the gate</span><span class="block text-xs text-muted-foreground">Off: an operator promotes after looking at the canary. On: it promotes itself once every canary target is healthy.</span></span>
          <Switch id="ro-auto" :model-value="strategy.auto_promote" @update:model-value="strategy.auto_promote = $event" />
        </label>
        <FormField label="Health grace" for="ro-grace" hint="How long a target has to pass the template’s health gate before it counts as failed." class="sm:col-span-2">
          <NumberField id="ro-grace" v-model="strategy.health_grace_secs" :min="30" :step="30" unit="s" class="w-40" />
        </FormField>
        <div class="rounded-xl border border-border bg-card p-3 sm:col-span-2">
          <p class="mb-3 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Compose options on every host</p>
          <DeployOptionsForm v-model="options" />
        </div>
      </div>

      <!-- Review -->
      <div v-else class="flex flex-col gap-4" data-testid="step-review">
        <FormField label="Rollout name" for="ro-name"><Input id="ro-name" v-model="name" data-testid="rollout-name" @input="nameTouched = true" /></FormField>
        <div class="rounded-xl border border-border bg-muted/40 p-3 text-sm">
          <p class="font-medium text-foreground"><Rocket class="mr-1 inline size-4 text-brand" /> {{ template?.name }} {{ version?.version }} → <template v-if="planned.length">{{ planned.length }} host{{ planned.length === 1 ? '' : 's' }} in {{ stages }} stage{{ stages === 1 ? '' : 's' }}</template><template v-else>no host can take it</template><span v-if="resolved.length > planned.length" class="font-normal text-muted-foreground"> · {{ resolved.length - planned.length }} skipped</span></p>
          <p class="mt-1 text-xs text-muted-foreground">{{ strategySummary(strategy) }}</p>
        </div>
        <div class="grid gap-3 sm:grid-cols-2" data-testid="rollout-plan">
          <div v-for="n in stages" :key="n" class="rounded-xl border border-border bg-card p-3" :data-plan-stage="n">
            <p class="text-xs font-medium text-foreground">Stage {{ n }} — {{ n === 1 && stages > 1 ? 'canary' : stages > 1 ? 'the rest' : 'all' }}</p>
            <ul class="mt-2 flex flex-col gap-1">
              <li v-if="!plan.some((x) => x.stage === n)" class="text-xs text-muted-foreground">no eligible host</li>
              <li v-for="p in plan.filter((x) => x.stage === n)" :key="p.host" class="flex items-center gap-2 text-xs" :data-preflight-host="p.host" :data-preflight="preflightTone(p.host)">
                <StatusPill :tone="preflightTone(p.host)" class="w-20 justify-center">{{ preflight[p.host] ? (preflight[p.host].ok ? (preflightTone(p.host) === 'warning' ? PREFLIGHT_META.warn.label : PREFLIGHT_META.pass.label) : 'Blocked') : preflightBusy ? '…' : '—' }}</StatusPill>
                <span class="shrink-0 whitespace-nowrap font-mono text-foreground">{{ p.host }}</span>
                <span class="min-w-0 truncate text-muted-foreground" :title="preflightNote(p.host)">{{ preflightNote(p.host) }}</span>
              </li>
            </ul>
          </div>
        </div>
        <p v-if="noCanary" class="rounded-lg border border-sev-error/30 bg-sev-error-soft px-3 py-2 text-xs text-sev-error" data-testid="no-canary">No host passes preflight, so there is nothing to roll out to. Fix the blocked hosts (disk, ports, agent) or pick other targets.</p>
        <p v-if="preflightFailed" class="text-xs text-sev-warn">{{ preflightFailed }} host{{ preflightFailed === 1 ? '' : 's' }} failed preflight and will be skipped (requirements not met) — not counted as failures.</p>
        <ComposeWarnings :warnings="warnings" :acknowledged="acknowledged" @update:acknowledged="acknowledged = $event" />
        <ul v-if="skipped.length" class="flex flex-col gap-1 text-xs text-muted-foreground" data-testid="rollout-skipped">
          <li v-for="s in skipped" :key="s.host"><span class="font-mono text-foreground">{{ s.host }}</span> — skipped: {{ s.why }}</li>
        </ul>
        <label class="flex items-center gap-2 text-sm" for="ro-start"><Switch id="ro-start" :model-value="startNow" @update:model-value="startNow = $event" /><span class="text-foreground">Start the canary now</span><span class="text-xs text-muted-foreground">{{ startNow ? '' : 'Off: saved as a draft to start later.' }}</span></label>
      </div>

      <p v-if="error" class="font-mono text-xs text-sev-error">{{ error }}</p>
      </div>

      <div class="flex shrink-0 items-center justify-between gap-2 border-t border-border bg-surface-2 px-6 py-3" data-testid="rollout-actions">
        <Button variant="ghost" :disabled="stepIndex === 0" @click="back"><ArrowLeft class="size-4" /> Back</Button>
        <div class="flex items-center gap-2">
          <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
          <Button v-if="step !== 'review'" :disabled="!canNext" data-testid="rollout-next" @click="next">Next <ArrowRight class="size-4" /></Button>
          <GatedButton v-else perm="deploy" variant="brand" :disabled="create.isPending.value || !name.trim() || unacked.length > 0 || preflightBusy || noCanary" :title="unacked.length ? `Acknowledge ${unacked.length} risky construct${unacked.length === 1 ? '' : 's'} first` : undefined" data-testid="rollout-submit" @click="submit"><Loader2 v-if="create.isPending.value" class="animate-spin" /><Rocket v-else class="size-4" /> {{ startNow ? 'Create and start' : 'Save draft' }}</GatedButton>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
