<script setup lang="ts">
// `/deploy/stacks` — every compose project under desired state, across hosts and Sites. Answers
// "what runs where, and is it what we applied": a strip of status chips that doubles as the
// filter (the Attention Board's world cards, in miniature), then search and host, then the list
// grouped under its hosts. Control-plane data, so no global time picker.
import { computed, ref } from 'vue'
import { Plus, Import, Search } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import StacksTable from '@/components/deploy/StacksTable.vue'
import NewStackDialog from '@/components/deploy/NewStackDialog.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { StatusDot } from '@/components/ui/status-dot'
import { Input } from '@/components/ui/input'
import { SelectMenu } from '@/components/ui/select-menu'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import { useUrlParam } from '@/lib/core/useUrlState'
import { useStacks, useAgents, useTemplates } from '@/lib/deploy/deployQueries'
import { useTenants } from '@/lib/tenants/tenantsQueries'
import { stackCounts, filterStacks, sortStacks } from '@/lib/deploy/stacks'
import { formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

const q = useStacks()
const all = computed(() => q.data.value?.stacks ?? [])
const loading = computed(() => q.isLoading.value)
const counts = computed(() => stackCounts(all.value))
const unavailable = computed(() => isDeployUnavailable(q.error.value))

const text = ref<string | null>(null)
const host = ref<string | null>(null)
const attention = ref<string | null>(null)
const bucket = ref<string | null>(null)
useUrlParam('q', text)
useUrlParam('host', host)
useUrlParam('attention', attention, { valid: (v) => v === '1' })
useUrlParam('show', bucket, { valid: (v) => v === 'running' || v === 'pending' })

// The chip strip: one bucket at a time. `attention` keeps its own URL param (`?attention=1` is
// what links from elsewhere use); running / pending ride on `?show=`.
type Chip = 'all' | 'attention' | 'pending' | 'running'
const chip = computed<Chip>(() => (attention.value === '1' ? 'attention' : bucket.value === 'running' || bucket.value === 'pending' ? bucket.value : 'all'))
function pick(c: Chip): void {
  attention.value = c === 'attention' ? '1' : null
  bucket.value = c === 'running' || c === 'pending' ? c : null
}
const hostCount = computed(() => new Set(all.value.filter((s) => s.status !== 'gone').map((s) => s.host)).size)
type Tone = 'success' | 'error' | 'warning' | 'neutral'
const chips = computed<{ key: Chip; n: number; label: string; sub: string; tone: Tone | null }[]>(() => {
  const c = counts.value
  return [
    { key: 'all', n: c.total, label: c.total === 1 ? 'stack' : 'stacks', sub: `${hostCount.value} ${hostCount.value === 1 ? 'host' : 'hosts'}`, tone: null },
    { key: 'attention', n: c.failed + c.drifted, label: 'Needs attention', sub: `${c.failed} failed · ${c.drifted} drifted`, tone: c.failed ? 'error' : c.drifted ? 'warning' : 'neutral' },
    { key: 'pending', n: c.pending, label: 'pending', sub: 'waiting for a host, or applying', tone: 'neutral' },
    { key: 'running', n: c.running, label: 'running', sub: 'applied and matching', tone: c.running ? 'success' : 'neutral' },
  ]
})

// The agent's state per host row, and each template's latest version so a stack behind it can say so.
const agentsQ = useAgents()
const agents = computed(() => agentsQ.data.value?.hosts ?? [])
const templatesQ = useTemplates()
const latestVersions = computed(() => Object.fromEntries((templatesQ.data.value?.templates ?? []).map((t) => [t.id, t.latest_version])))

// Site narrowing is the ContextBar's tenant picker — the one dimension every page shares.
const tenantsQ = useTenants()
const hasSites = computed(() => (tenantsQ.data.value?.tenants?.length ?? 0) > 0 || all.value.some((s) => !!s.site))
const hostOptions = computed(() => [{ value: '', label: 'Any host' }, ...Array.from(new Set(all.value.map((s) => s.host))).sort().map((h) => ({ value: h, label: h }))])

const rows = computed(() => sortStacks(filterStacks(all.value, { q: text.value, host: host.value, attention: attention.value === '1', bucket: bucket.value === 'running' || bucket.value === 'pending' ? bucket.value : null })))

const dialogOpen = ref(false)
const dialogMode = ref<'compose' | 'adopt'>('compose')
function openNew(mode: 'compose' | 'adopt'): void {
  dialogMode.value = mode
  dialogOpen.value = true
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Deploy › Stacks" :time-picker="false">
    <template #actions>
      <GatedButton perm="deploy" size="sm" variant="outline" data-testid="adopt-stack" @click="openNew('adopt')"><Import class="size-4" /> Adopt existing</GatedButton>
      <GatedButton perm="compose" size="sm" data-testid="new-stack" @click="openNew('compose')"><Plus class="size-4" /> New stack</GatedButton>
    </template>
    <section class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 pt-5 pb-6" data-testid="stacks-view">
      <header class="flex items-start justify-between gap-4">
        <div>
          <h1 class="text-xl font-semibold text-foreground">Stacks</h1>
          <p class="text-sm text-muted-foreground">Compose projects under desired state, one per host. What runs at each site, and which recipe produced it.</p>
        </div>
      </header>

      <!-- The count band as a filter: click a bucket to see only it. -->
      <div class="flex flex-wrap items-center gap-2" data-testid="stack-kpis">
        <div class="flex flex-wrap gap-2" data-testid="stack-attention">
          <button
            v-for="c in chips"
            :key="c.key"
            type="button"
            :class="cn('inline-flex h-9 items-center gap-2 rounded-xl border px-3 text-sm transition-colors', chip === c.key ? 'border-brand bg-brand-soft text-foreground' : 'border-border bg-card text-foreground hover:border-foreground/20')"
            :aria-pressed="chip === c.key"
            :data-chip="c.key"
            @click="pick(c.key)"
          >
            <StatusDot v-if="c.tone" :tone="c.tone" size="sm" />
            <span class="font-semibold tabular-nums">{{ formatNumber(c.n) }}</span>
            <span>{{ c.label }}</span>
            <span class="hidden text-xs text-muted-foreground lg:inline">· {{ c.sub }}</span>
          </button>
        </div>
        <div class="ml-auto flex flex-wrap items-center gap-2" data-testid="stack-filters">
          <div class="relative w-64">
            <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input :model-value="text ?? ''" placeholder="Search stacks, hosts, services…" class="h-9 pl-8 text-xs" data-testid="stack-search" @update:model-value="text = String($event) || null" />
          </div>
          <SelectMenu :model-value="host ?? ''" :options="hostOptions" prefix="Host" content-class="w-44" @update:model-value="host = String($event) || null" />
        </div>
      </div>

      <DeployUnavailable v-if="unavailable" what="Stacks" />
      <div v-else-if="loading && !all.length" class="flex flex-1 items-center justify-center py-16"><Spinner size="lg">Loading stacks…</Spinner></div>
      <StacksTable v-else :stacks="rows" :loading="loading" :show-site="hasSites" :agents="agents" :latest-versions="latestVersions" />

      <NewStackDialog :open="dialogOpen" :mode="dialogMode" @update:open="dialogOpen = $event" />
    </section>
  </AppShell>
</template>
