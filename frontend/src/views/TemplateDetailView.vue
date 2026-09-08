<script setup lang="ts">
// `/deploy/templates/:id` — one template: what it is, what it needs, its typed variables, its
// immutable versions, the files, and where it runs ("which version is customer X on" is the
// Deployments tab). Deploy any version from here; publish the next one.
import { computed, ref } from 'vue'
import { useRoute, useRouter, RouterLink } from 'vue-router'
import { ArrowLeft, Rocket, Upload, Activity, ShieldCheck, Layers } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import TemplateIcon from '@/components/deploy/TemplateIcon.vue'
import DeployTemplateDialog from '@/components/deploy/DeployTemplateDialog.vue'
import PublishVersionDialog from '@/components/deploy/PublishVersionDialog.vue'
import NewRolloutDialog from '@/components/deploy/NewRolloutDialog.vue'
import TemplateVersionMatrix from '@/components/deploy/TemplateVersionMatrix.vue'
import VersionDiff from '@/components/deploy/VersionDiff.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { api } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import { backTo } from '@/lib/core/useBackTo'
import { useTemplate, useTemplateVersion, useStacks } from '@/lib/deploy/deployQueries'
import { VARIABLE_TYPE_META, manifestToYaml } from '@/lib/deploy/templates'
import { ageMs, stampMs } from '@/lib/deploy/audit'
import { formatBytes } from '@/lib/core/format'

const route = useRoute()
const router = useRouter()
const id = computed(() => String(Array.isArray(route.params.id) ? route.params.id[0] : route.params.id ?? ''))
const q = useTemplate(id)
const template = computed(() => q.data.value ?? null)
const loading = computed(() => q.isLoading.value)

const TABS = ['overview', 'variables', 'versions', 'files', 'deployments'] as const
type Tab = (typeof TABS)[number]
const TAB_LABEL: Record<Tab, string> = { overview: 'Overview', variables: 'Variables', versions: 'Versions', files: 'Files', deployments: 'Deployments' }
const tab = computed<Tab>(() => ((TABS as readonly string[]).includes(String(route.query.tab)) ? (route.query.tab as Tab) : 'overview'))

// The version being looked at (Files / Variables): `?v=` or the latest.
const selectedVersionId = computed(() => (typeof route.query.v === 'string' && route.query.v) || template.value?.versions[0]?.id || null)
const versionQ = useTemplateVersion(selectedVersionId)
const version = computed(() => versionQ.data.value ?? null)
const manifest = computed(() => version.value?.manifest ?? null)

const stacksQ = useStacks()
const deployments = computed(() => (stacksQ.data.value?.stacks ?? []).filter((s) => s.template_id === id.value && s.status !== 'gone'))

const deployOpen = ref(false)
const deployVersion = ref<string | null>(null)
const publishOpen = ref(false)
const rolloutOpen = ref(false)
// Versions tab: which version is being compared with the one before it.
const compareId = ref<string | null>(null)
function deploy(versionId: string | null = null): void {
  deployVersion.value = versionId
  deployOpen.value = true
}
function back(): void {
  backTo(router, '/deploy/templates')
}
const HEALTH_LABEL = { container_healthy: 'container health checks', uptime_monitor: 'an uptime monitor', slo_burn: 'an SLO burn-rate check' }
</script>

<template>
  <AppShell :mock="api.mock" :crumb="`Deploy › Templates › ${template?.name ?? '…'}`" :time-picker="false">
    <template #lead>
      <Button size="icon" variant="ghost" class="size-7 shrink-0" aria-label="Back to templates" @click="back"><ArrowLeft class="size-4" /></Button>
    </template>
    <template #toolbar>
      <NavTabs v-if="template" class="text-xs" aria-label="Template sections">
        <NavTabItem v-for="t in TABS" :key="t" :to="{ query: { ...route.query, tab: t === 'overview' ? undefined : t } }" :active="tab === t" :data-tab="t">{{ TAB_LABEL[t] }}</NavTabItem>
      </NavTabs>
    </template>
    <template #actions>
      <GatedButton v-if="template" perm="templates" size="sm" variant="outline" data-testid="publish-version" @click="publishOpen = true"><Upload class="size-4" /> Publish version</GatedButton>
      <GatedButton v-if="template" perm="deploy" size="sm" variant="outline" data-testid="rollout-template" @click="rolloutOpen = true"><Layers class="size-4" /> Roll out to many</GatedButton>
      <GatedButton v-if="template" perm="deploy" size="sm" variant="brand" data-testid="deploy-template" @click="deploy()"><Rocket class="size-4" /> Deploy {{ template.latest_version }}</GatedButton>
    </template>

    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="template-detail">
      <DeployUnavailable v-if="isDeployUnavailable(q.error.value)" what="The template catalog" />
      <div v-else-if="loading && !template" class="flex flex-1 items-center justify-center py-16"><Spinner size="lg">Loading template…</Spinner></div>
      <Card v-else-if="!template" class="py-4"><EmptyState title="No such template"><RouterLink to="/deploy/templates" class="mt-3 text-sm text-brand hover:underline">Back to templates →</RouterLink></EmptyState></Card>
      <template v-else>
        <Card class="flex flex-wrap items-start gap-4 p-4" data-testid="template-header">
          <TemplateIcon :icon="template.icon" :name="template.name" size="lg" />
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <h1 class="text-lg font-semibold text-foreground">{{ template.name }}</h1>
              <Badge variant="brand" class="font-mono">{{ template.latest_version }}</Badge>
              <Badge v-if="template.source !== 'local'" variant="outline">{{ template.source }}</Badge>
              <Badge v-for="c in template.categories" :key="c" variant="secondary" class="font-normal">{{ c }}</Badge>
            </div>
            <p class="mt-1 text-sm text-muted-foreground">{{ template.description || 'No description.' }}</p>
            <p class="mt-2 text-xs text-muted-foreground"><span class="font-mono">{{ template.id }}</span> · {{ template.versions.length }} {{ template.versions.length === 1 ? 'version' : 'versions' }} · running on {{ template.stacks }} {{ template.stacks === 1 ? 'host' : 'hosts' }} · updated {{ ageMs(template.updated_ms) }}</p>
          </div>
        </Card>

        <!-- Overview -->
        <template v-if="tab === 'overview'">
          <div v-if="manifest" class="grid gap-4 lg:grid-cols-2">
            <Card class="p-4" data-testid="template-requires">
              <h3 class="text-sm font-medium text-foreground">What a host needs</h3>
              <dl class="mt-2 grid grid-cols-[8rem_1fr] gap-y-1 text-xs">
                <dt class="text-muted-foreground">Architecture</dt><dd class="font-mono text-foreground">{{ manifest.requires.arch?.join(', ') || 'any' }}</dd>
                <dt class="text-muted-foreground">Docker</dt><dd class="font-mono text-foreground">{{ manifest.requires.min_docker ? `≥ ${manifest.requires.min_docker}` : 'any' }}</dd>
                <dt class="text-muted-foreground">Memory</dt><dd class="text-foreground">{{ manifest.requires.min_ram_bytes ? `≥ ${formatBytes(manifest.requires.min_ram_bytes)}` : '—' }}</dd>
                <dt class="text-muted-foreground">Free disk</dt><dd class="text-foreground">{{ manifest.requires.min_disk_bytes ? `≥ ${formatBytes(manifest.requires.min_disk_bytes)}` : '—' }}</dd>
                <dt class="text-muted-foreground">Ports</dt><dd class="flex flex-wrap gap-1"><Badge v-for="p in manifest.ports" :key="p.name" variant="outline" class="font-mono">{{ p.default_host }}→{{ p.container }} {{ p.name }}</Badge><span v-if="!manifest.ports.length" class="text-muted-foreground">none published</span></dd>
                <dt class="text-muted-foreground">Volumes</dt><dd class="flex flex-wrap gap-1"><Badge v-for="v in manifest.volumes" :key="v.name" variant="outline" class="font-mono" :title="v.description ?? undefined">{{ v.name }} → {{ v.path }}</Badge><span v-if="!manifest.volumes.length" class="text-muted-foreground">none</span></dd>
              </dl>
              <p class="mt-3 text-[11px] text-muted-foreground">Preflight checks each of these against the target host before a deploy.</p>
            </Card>
            <Card class="p-4" data-testid="template-observability">
              <h3 class="flex items-center gap-2 text-sm font-medium text-foreground"><Activity class="size-4 text-brand" /> Observable from the first second</h3>
              <p class="mt-1 text-xs text-muted-foreground">The Photon block injects into the deployed services:</p>
              <ul class="mt-2 flex flex-col gap-1 text-xs">
                <li :class="manifest.observability.otlp_endpoint ? 'text-foreground' : 'text-muted-foreground/60 line-through'">OTLP endpoint (<code class="font-mono">OTEL_EXPORTER_OTLP_ENDPOINT</code>) pointing at this Photon</li>
                <li :class="manifest.observability.ingest_token ? 'text-foreground' : 'text-muted-foreground/60 line-through'">an ingest token, so nothing at the site needs configuring</li>
                <li :class="manifest.observability.environment ? 'text-foreground' : 'text-muted-foreground/60 line-through'"><code class="font-mono">deployment.environment</code> from the variables, for the environment picker</li>
                <li class="text-foreground"><code class="font-mono">service.name</code>: {{ manifest.observability.service_name ?? 'each compose service keeps its own' }}</li>
              </ul>
              <div class="mt-3 flex items-start gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs">
                <ShieldCheck class="mt-0.5 size-3.5 shrink-0 text-success" />
                <span class="text-foreground">Health gate: <template v-if="manifest.health">{{ HEALTH_LABEL[manifest.health.kind] }} — {{ manifest.health.detail }}, {{ manifest.health.grace_secs }}s grace. A deploy is "done" when this passes, and a rollout's canary is judged by it.</template><template v-else>none declared — a deploy counts as done when the apply succeeds.</template></span>
              </div>
            </Card>
          </div>
        </template>

        <!-- Variables -->
        <Card v-else-if="tab === 'variables'" class="overflow-hidden p-0" data-testid="template-variables">
          <Table class="text-sm">
            <TableHeader><TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent"><TableHead>Variable</TableHead><TableHead>Type</TableHead><TableHead>Default</TableHead><TableHead>Rules</TableHead><TableHead>Group</TableHead></TableRow></TableHeader>
            <TableBody>
              <TableRow v-for="v in manifest?.variables ?? []" :key="v.name" class="border-border/60" :data-variable="v.name">
                <TableCell class="py-2"><div class="font-mono text-xs text-foreground">{{ v.name }}</div><div class="text-xs text-muted-foreground">{{ v.label }}<template v-if="v.description"> — {{ v.description }}</template></div></TableCell>
                <TableCell class="py-2 text-xs text-foreground" :title="VARIABLE_TYPE_META[v.type].summary">{{ VARIABLE_TYPE_META[v.type].label }}</TableCell>
                <TableCell class="py-2 font-mono text-xs text-muted-foreground">{{ v.type === 'generated' ? '(minted)' : v.type === 'secret' ? '(secret)' : (v.default ?? '—') }}</TableCell>
                <TableCell class="py-2 text-xs text-muted-foreground">{{ [v.required ? 'required' : null, v.preset ? 'preset' : null, v.options?.length ? v.options.join(' | ') : null, v.pattern ? `/${v.pattern}/` : null].filter(Boolean).join(' · ') || '—' }}</TableCell>
                <TableCell class="py-2 text-xs text-muted-foreground">{{ v.group ?? '—' }}</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </Card>

        <!-- Versions -->
        <Card v-else-if="tab === 'versions'" class="divide-y divide-border p-0" data-testid="template-versions">
          <div v-for="(v, i) in template.versions" :key="v.id" class="flex flex-wrap items-center gap-3 px-4 py-3" :data-version="v.version">
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-2"><span class="font-mono text-sm font-medium text-foreground">{{ v.version }}</span><Badge v-if="i === 0" variant="brand">latest</Badge><span class="text-xs text-muted-foreground">{{ stampMs(v.created_ms) }} by {{ v.created_by }}</span><span v-if="v.deployed_count" class="text-xs text-muted-foreground">· on {{ v.deployed_count }} {{ v.deployed_count === 1 ? 'host' : 'hosts' }}</span></div>
              <p class="text-xs text-muted-foreground">{{ v.notes ?? 'No release notes.' }}</p>
            </div>
            <button v-if="i < template.versions.length - 1" type="button" class="text-xs text-brand hover:underline" :data-compare="v.version" @click="compareId = compareId === v.id ? null : v.id">{{ compareId === v.id ? 'Hide diff' : `Diff vs ${template.versions[i + 1].version}` }}</button>
            <RouterLink :to="{ query: { ...route.query, tab: 'files', v: v.id } }" class="text-xs text-brand hover:underline">Files</RouterLink>
            <GatedButton perm="deploy" size="sm" variant="outline" class="h-7 px-2 text-xs" :data-deploy-version="v.version" @click="deploy(v.id)"><Rocket class="size-3.5" /> Deploy {{ v.version }}</GatedButton>
            <div v-if="compareId === v.id && i < template.versions.length - 1" class="w-full"><VersionDiff :from-id="template.versions[i + 1].id" :to-id="v.id" :from-label="template.versions[i + 1].version" :to-label="v.version" /></div>
          </div>
        </Card>

        <!-- Files -->
        <template v-else-if="tab === 'files'">
          <div class="flex items-center gap-2 text-xs text-muted-foreground">Version <span class="font-mono text-foreground">{{ version?.version ?? '…' }}</span> — immutable; the next change is a new version.</div>
          <div v-if="version" class="grid gap-4 lg:grid-cols-2">
            <Card class="overflow-hidden p-0"><div class="border-b border-border px-3 py-1.5 font-mono text-xs text-muted-foreground">compose.yaml</div><pre class="max-h-[32rem] overflow-auto p-3 font-mono text-[11.5px] leading-5 text-foreground" data-testid="template-compose">{{ version.compose_text }}</pre></Card>
            <Card class="overflow-hidden p-0"><div class="border-b border-border px-3 py-1.5 font-mono text-xs text-muted-foreground">photon.template.yaml</div><pre class="max-h-[32rem] overflow-auto p-3 font-mono text-[11.5px] leading-5 text-foreground" data-testid="template-manifest">{{ manifestToYaml(version.manifest) }}</pre></Card>
          </div>
        </template>

        <!-- Deployments -->
        <template v-else>
          <p class="text-xs text-muted-foreground">Every host running this template, the version it is on, and the one click that brings the stragglers up.</p>
          <TemplateVersionMatrix :template="template" :stacks="deployments" />
        </template>

        <DeployTemplateDialog :open="deployOpen" :template-id="template.id" :version-id="deployVersion" @update:open="deployOpen = $event" />
        <PublishVersionDialog :open="publishOpen" :template="template" @update:open="publishOpen = $event" />
        <NewRolloutDialog :open="rolloutOpen" :template-id="template.id" @update:open="rolloutOpen = $event" />
      </template>
    </main>
  </AppShell>
</template>
