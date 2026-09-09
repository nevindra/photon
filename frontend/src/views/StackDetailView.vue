<script setup lang="ts">
// `/deploy/stacks/:id` — one stack on one host. Header answers status / where / which recipe /
// which generation, in one line; the action bar carries the compose verbs with their confirmations,
// and drift is a row of that same header card. The tabs are Overview (the services as observed,
// each with its drift and its APM / logs links on the row, then the verdict), Files (the
// editor — every save is a new generation, applied deliberately), History (its audit rows).
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter, RouterLink } from 'vue-router'
import { ArrowLeft, ArrowUpCircle, Play, RotateCw, Download, Square, Trash2, MoreHorizontal, Server, Save, Loader2 } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import StackStatusPill from '@/components/deploy/StackStatusPill.vue'
import StackServicesGrid from '@/components/deploy/StackServicesGrid.vue'
import StackDriftPanel from '@/components/deploy/StackDriftPanel.vue'
import StackActionDialog from '@/components/deploy/StackActionDialog.vue'
import ComposeEditor from '@/components/deploy/ComposeEditor.vue'
import DeployOptionsForm from '@/components/deploy/DeployOptionsForm.vue'
import AuditTable from '@/components/deploy/AuditTable.vue'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import DeployTemplateDialog from '@/components/deploy/DeployTemplateDialog.vue'
import DeployImpactPanel from '@/components/deploy/DeployImpactPanel.vue'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator } from '@/components/ui/dropdown-menu'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import { api, type DeployOptions, type StackActionArgs, type StackActionKind, type StackFile } from '@/lib/core/api'
import { backTo } from '@/lib/core/useBackTo'
import { can } from '@/lib/core/auth'
import { permissionHint } from '@/lib/deploy/roles'
import { useStack, useAgent, useStackAction, useUpdateStackFiles, useAudit, useTemplate } from '@/lib/deploy/deployQueries'
import { semverCompare } from '@/lib/deploy/templates'
import { STACK_STATUS_META, generationLabel, hasPendingGeneration, sourceLabel } from '@/lib/deploy/stacks'
import { ageMs, stampMs } from '@/lib/deploy/audit'

const route = useRoute()
const router = useRouter()
const id = computed(() => String(Array.isArray(route.params.id) ? route.params.id[0] : route.params.id ?? ''))
const q = useStack(id)
const stack = computed(() => q.data.value ?? null)
const loading = computed(() => q.isLoading.value)
const agentQ = useAgent(() => stack.value?.host ?? '')
const agent = computed(() => agentQ.data.value ?? null)
const hostStatus = computed(() => agent.value?.status ?? 'pending')

const TABS = ['overview', 'files', 'history'] as const
type Tab = (typeof TABS)[number]
const TAB_LABEL: Record<Tab, string> = { overview: 'Overview', files: 'Files', history: 'History' }
const tab = computed<Tab>(() => ((TABS as readonly string[]).includes(String(route.query.tab)) ? (route.query.tab as Tab) : 'overview'))

const blocked = computed<string | null>(() => {
  if (!can('deploy')) return permissionHint('deploy')
  if (agent.value && !agent.value.capabilities.includes('apply')) return 'The agent on this host was installed without --docker-apply'
  if (stack.value && (stack.value.status === 'applying' || stack.value.status === 'destroying')) return 'An apply is in progress'
  return null
})
const busy = computed(() => !!stack.value && (stack.value.status === 'applying' || stack.value.status === 'destroying' || stack.value.status === 'pending'))

// --- Template upgrades: the catalog has a newer version than this stack pins. ---
const templateQ = useTemplate(() => stack.value?.template_id ?? '')
const newerVersion = computed(() => {
  const t = templateQ.data.value
  const s = stack.value
  if (!t || !s?.template_version) return null
  const latest = t.versions[0]
  return latest && semverCompare(latest.version, s.template_version) > 0 ? latest : null
})
const upgradeOpen = ref(false)

// --- Actions ---
const action = useStackAction()
const dialogOpen = ref(false)
const dialogKind = ref<StackActionKind | null>(null)
function ask(kind: StackActionKind): void {
  if (blocked.value) return
  dialogKind.value = kind
  dialogOpen.value = true
}
async function onConfirm(args: StackActionArgs): Promise<void> {
  if (!stack.value || !dialogKind.value) return
  const res = await action.mutateAsync({ id: stack.value.id, kind: dialogKind.value, args })
  if (res.ok) dialogOpen.value = false
}

// --- Files ---
const files = ref<StackFile[]>([])
const acknowledged = ref<string[]>([])
const options = ref<DeployOptions>({ pull: 'missing', force_recreate: false, recreate_volumes: false })
const editor = ref<InstanceType<typeof ComposeEditor> | null>(null)
// Seed the editor from the stack once per (stack, generation): a poll that returns the same
// generation must not clobber an edit in progress, while a save or a different stack must.
const seeded = ref('')
watch(stack, (s) => {
  if (!s) return
  const key = `${s.id}:${s.desired_generation}`
  if (seeded.value === key) return
  seeded.value = key
  files.value = s.files.map((f) => ({ ...f }))
  acknowledged.value = [...s.acknowledged_warnings]
  options.value = { ...s.deploy_options }
}, { immediate: true })
const dirty = computed(() => !!stack.value && (JSON.stringify(files.value) !== JSON.stringify(stack.value.files) || JSON.stringify(options.value) !== JSON.stringify(stack.value.deploy_options) || JSON.stringify([...acknowledged.value].sort()) !== JSON.stringify([...stack.value.acknowledged_warnings].sort())))
const save = useUpdateStackFiles()
const saveError = ref('')
async function doSave(): Promise<void> {
  if (!stack.value) return
  saveError.value = ''
  const res = await save.mutateAsync({ id: stack.value.id, files: files.value, acknowledged: acknowledged.value, deployOptions: options.value })
  if (!res.ok) saveError.value = res.error ?? 'Could not save.'
}
function discard(): void {
  if (!stack.value) return
  files.value = stack.value.files.map((f) => ({ ...f }))
  acknowledged.value = [...stack.value.acknowledged_warnings]
  options.value = { ...stack.value.deploy_options }
}

// --- History ---
const auditQ = useAudit(computed(() => ({ host: stack.value?.host, q: stack.value?.name, limit: 100 })))
const history = computed(() => (auditQ.data.value?.entries ?? []).filter((e) => e.target.stack === stack.value?.name))

function back(): void {
  backTo(router, '/deploy/stacks')
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="`Deploy › Stacks › ${stack?.name ?? '…'}`" :time-picker="false">
    <template #lead>
      <Button size="icon" variant="ghost" class="size-7 shrink-0" aria-label="Back to stacks" data-testid="back-to-stacks" @click="back"><ArrowLeft class="size-4" /></Button>
    </template>
    <template #toolbar>
      <NavTabs v-if="stack" class="text-xs" aria-label="Stack sections">
        <NavTabItem v-for="t in TABS" :key="t" :to="{ query: { ...route.query, tab: t === 'overview' ? undefined : t } }" :active="tab === t" :data-tab="t">{{ TAB_LABEL[t] }}</NavTabItem>
      </NavTabs>
    </template>

    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="stack-detail">
      <DeployUnavailable v-if="isDeployUnavailable(q.error.value)" what="Stacks" />
      <div v-else-if="loading && !stack" class="flex flex-1 items-center justify-center py-16"><Spinner size="lg">Loading stack…</Spinner></div>
      <Card v-else-if="!stack" class="py-4"><EmptyState title="No such stack" description="It may have been destroyed, or the link is stale."><RouterLink to="/deploy/stacks" class="mt-3 text-sm text-brand hover:underline">Back to stacks →</RouterLink></EmptyState></Card>

      <template v-else>
        <!-- Header: the name and its status, ONE line of facts, the actions — and, when drifted,
             the drift as a row of the same card, so the state and its cause are read together. -->
        <Card class="overflow-hidden p-0" data-testid="stack-header">
          <div class="flex flex-wrap items-start gap-4 p-4">
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-2">
                <h1 class="text-lg font-semibold text-foreground">{{ stack.name }}</h1>
                <StackStatusPill :status="stack.status" solid :title="STACK_STATUS_META[stack.status].hint" />
                <Badge v-if="stack.source === 'template'" variant="brand">template · {{ sourceLabel(stack) }}</Badge>
                <Badge v-else variant="outline">{{ sourceLabel(stack) }}</Badge>
                <Badge v-if="newerVersion" variant="warning" data-testid="upgrade-available">{{ newerVersion.version }} available</Badge>
              </div>
              <p class="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground" data-testid="stack-facts">
                <span>on <RouterLink :to="`/infra/${encodeURIComponent(stack.host)}/containers`" class="inline-flex items-center gap-1 font-mono text-foreground hover:underline"><Server class="size-3 text-muted-foreground" />{{ stack.host }}</RouterLink><template v-if="stack.site"> · {{ stack.site }}</template></span>
                <span>generation <span class="font-mono tabular-nums" :class="hasPendingGeneration(stack) ? 'text-brand' : 'text-foreground'">{{ generationLabel(stack) }}</span><template v-if="hasPendingGeneration(stack) && stack.status !== 'applying' && stack.status !== 'pending'"> rendered, not applied</template></span>
                <span>applied <span class="text-foreground">{{ stack.last_applied_ms ? `${ageMs(stack.last_applied_ms)} · ${stampMs(stack.last_applied_ms)}` : 'never' }}</span></span>
                <span>created <span class="text-foreground">{{ stampMs(stack.created_ms) }}</span> by <span class="text-foreground">{{ stack.created_by }}</span></span>
              </p>
            </div>
            <div class="flex flex-wrap items-center gap-2" data-testid="stack-actions">
              <GatedButton v-if="newerVersion && stack.status !== 'gone'" perm="deploy" size="sm" variant="brand" :disabled="!!blocked" :reason="blocked" data-testid="stack-upgrade" @click="upgradeOpen = true"><ArrowUpCircle class="size-4" /> Upgrade to {{ newerVersion.version }}</GatedButton>
              <GatedButton v-if="stack.status !== 'gone'" perm="deploy" size="sm" :variant="!newerVersion && (hasPendingGeneration(stack) || stack.status === 'failed' || stack.status === 'declared' || stack.status === 'rendered') ? 'brand' : 'outline'" :disabled="!!blocked" :reason="blocked" data-action="up" @click="ask('up')">
                <Loader2 v-if="busy" class="animate-spin" /><Play v-else class="size-4" /> {{ busy ? STACK_STATUS_META[stack.status].label : hasPendingGeneration(stack) ? `Apply generation ${stack.desired_generation}` : 'Apply' }}
              </GatedButton>
              <GatedButton v-if="stack.status === 'running' || stack.status === 'drifted'" perm="deploy" size="sm" variant="outline" :disabled="!!blocked" :reason="blocked" data-action="restart" @click="ask('restart')"><RotateCw class="size-4" /> Restart</GatedButton>
              <GatedButton v-if="stack.status === 'running' || stack.status === 'drifted'" perm="deploy" size="sm" variant="outline" :disabled="!!blocked" :reason="blocked" data-action="redeploy" @click="ask('redeploy')"><Download class="size-4" /> Redeploy</GatedButton>
              <DropdownMenu v-if="stack.status !== 'gone'">
                <DropdownMenuTrigger as-child><Button variant="outline" size="icon" class="size-8" aria-label="More stack actions" data-testid="stack-more"><MoreHorizontal class="size-4" /></Button></DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="w-48">
                  <DropdownMenuItem :disabled="!!blocked || stack.status === 'declared'" data-action="down" @select="ask('down')"><Square class="size-4" /> Take down</DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem :disabled="!!blocked" class="text-sev-error focus:text-sev-error" data-action="destroy" @select="ask('destroy')"><Trash2 class="size-4" /> Destroy…</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
          <p v-if="stack.last_error" class="border-t border-sev-error/30 bg-sev-error-soft px-4 py-2 font-mono text-xs text-sev-error" data-testid="stack-error">{{ stack.last_error }}</p>
          <StackDriftPanel v-if="tab === 'overview'" :drift="stack.drift" :blocked="blocked" @reconcile="ask('reconcile')" @accept="ask('accept')" />
        </Card>

        <!-- Overview: the services (with their drift and their observability links on the row) and the verdict. -->
        <template v-if="tab === 'overview'">
          <StackServicesGrid :stack="stack" />
          <DeployImpactPanel :services="stack.apm_services" :applied-ms="stack.last_applied_ms" />
        </template>

        <!-- Files -->
        <template v-else-if="tab === 'files'">
          <ComposeEditor ref="editor" :files="files" :acknowledged="acknowledged" :secrets="stack.secrets" :readonly="!can('compose') || stack.status === 'gone'" :server-error="saveError || null" @update:files="files = $event" @update:acknowledged="acknowledged = $event" />
          <Card class="p-4">
            <p class="mb-3 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Deploy options</p>
            <DeployOptionsForm v-model="options" />
          </Card>
          <div class="flex flex-wrap items-center gap-2" data-testid="files-footer">
            <span class="text-xs text-muted-foreground">{{ dirty ? 'Unsaved changes. Saving renders a new generation; apply it from the header when ready.' : `Generation ${stack.desired_generation} · secrets referenced: ${stack.secrets.length ? stack.secrets.join(', ') : 'none'}` }}</span>
            <div class="ml-auto flex items-center gap-2">
              <Button v-if="dirty" variant="ghost" size="sm" @click="discard">Discard</Button>
              <GatedButton perm="compose" size="sm" :disabled="!dirty || !editor?.ready || save.isPending.value" data-testid="stack-save" @click="doSave"><Loader2 v-if="save.isPending.value" class="animate-spin" /><Save v-else class="size-4" /> Save as generation {{ stack.desired_generation + 1 }}</GatedButton>
            </div>
          </div>
        </template>

        <!-- History -->
        <template v-else>
          <AuditTable :entries="history" :loading="auditQ.isLoading.value" />
        </template>

        <StackActionDialog :open="dialogOpen" :kind="dialogKind" :stack="stack" :host-status="hostStatus" :pending="action.isPending.value" @update:open="dialogOpen = $event" @confirm="onConfirm" />
        <DeployTemplateDialog v-if="stack.template_id" :open="upgradeOpen" :template-id="stack.template_id" :version-id="newerVersion?.id ?? null" :stack="stack" @update:open="upgradeOpen = $event" />
      </template>
    </main>
  </AppShell>
</template>
