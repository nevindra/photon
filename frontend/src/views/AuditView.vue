<script setup lang="ts">
// `/audit` (Manage group): the audit log — who did what, to which host, when, and how it ended.
// Read-only for every role; it exists so that the first question after a bad deploy ("who changed
// what?") has an answer that is not somebody's memory.
//
// It does NOT follow the global time Window (`:time-picker="false"`): the log is control-plane
// history, not telemetry, and a 30-minute default window would show an empty page on a quiet
// fleet. It carries its own coarse "since" picker instead, and the whole filter set lives in URL
// params so a row can be linked.
import { computed, ref } from 'vue'
import { refDebounced } from '@vueuse/core'
import { Search } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import AuditTable from '@/components/deploy/AuditTable.vue'
import { Input } from '@/components/ui/input'
import { SelectMenu } from '@/components/ui/select-menu'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Spinner } from '@/components/ui/spinner'
import { api, type AuditFilter, type AuditResultKind } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import { useUrlParam } from '@/lib/core/useUrlState'
import { useAudit, useAgents } from '@/lib/deploy/deployQueries'
import { ACTION_FAMILIES, SINCE_OPTIONS, RESULTS, RESULT_META, sinceMs } from '@/lib/deploy/audit'
import { formatNumber } from '@/lib/core/format'

const since = ref<string | null>('7d')
const result = ref<string | null>(null)
const actor = ref<string | null>(null)
const family = ref<string | null>(null)
const host = ref<string | null>(null)
const q = ref<string | null>(null)
useUrlParam('since', since, { valid: (v) => SINCE_OPTIONS.some((o) => o.value === v), fallback: '7d' })
useUrlParam('result', result, { valid: (v) => (RESULTS as string[]).includes(v) })
useUrlParam('actor', actor)
useUrlParam('act', family)
useUrlParam('host', host)
useUrlParam('q', q)
// The text filter waits for a pause in typing; the pickers and result toggle are immediate.
const qDebounced = refDebounced(q, 300)

const filter = computed<AuditFilter>(() => ({
  since_ms: sinceMs(since.value ?? '7d'),
  result: (result.value as AuditResultKind) || undefined,
  actor: actor.value || undefined,
  action: family.value || undefined,
  host: host.value || undefined,
  q: qDebounced.value?.trim() || undefined,
  limit: 200,
}))
const auditQ = useAudit(filter)
const entries = computed(() => auditQ.data.value?.entries ?? [])
const total = computed(() => auditQ.data.value?.total ?? 0)
const loading = computed(() => auditQ.isLoading.value)
const unavailable = computed(() => isDeployUnavailable(auditQ.error.value))

// Actor / host pickers are fed from the unfiltered corpus, so narrowing by one never empties the
// other's list — a picker that only offers what you already picked cannot change anything.
const allQ = useAudit(computed<AuditFilter>(() => ({ since_ms: sinceMs(since.value ?? '7d'), limit: 300 })))
const actors = computed(() => Array.from(new Set((allQ.data.value?.entries ?? []).map((e) => e.actor))).sort())
const agentsQ = useAgents()
const hosts = computed(() => {
  const fromAudit = (allQ.data.value?.entries ?? []).map((e) => e.target.host).filter((h): h is string => !!h)
  const fromAgents = (agentsQ.data.value?.hosts ?? []).map((h) => h.host)
  return Array.from(new Set([...fromAudit, ...fromAgents])).sort()
})

const counts = computed(() => {
  const c: Record<AuditResultKind, number> = { ok: 0, failed: 0, denied: 0, pending: 0 }
  for (const e of entries.value) c[e.result] += 1
  return c
})

const withAll = (opts: { value: string; label: string }[], allLabel: string) => [{ value: '', label: allLabel }, ...opts]
const sinceOpts = SINCE_OPTIONS.map((o) => ({ value: o.value, label: o.label }))
const familyOpts = withAll(ACTION_FAMILIES, 'All actions')
const actorOpts = computed(() => withAll(actors.value.map((a) => ({ value: a, label: a })), 'Anyone'))
const hostOpts = computed(() => withAll(hosts.value.map((h) => ({ value: h, label: h })), 'Any host'))

function onResult(v: unknown): void {
  result.value = typeof v === 'string' && v ? v : null
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Audit" :time-picker="false">
    <section class="flex min-h-0 flex-1 flex-col overflow-y-auto px-5 pt-5 pb-6" data-testid="audit-view">
      <header class="mb-5 flex items-start justify-between gap-4">
        <div>
          <h1 class="text-xl font-semibold text-foreground">Audit log</h1>
          <p class="text-sm text-muted-foreground">
            Every change made through Photon to a host — who, what, where, and how it ended. Kept independently of telemetry retention.
          </p>
        </div>
      </header>

      <div class="mb-4 flex flex-wrap items-center gap-2" data-testid="audit-filters">
        <div class="relative min-w-[16rem] flex-1">
          <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            :model-value="q ?? ''"
            placeholder="Search actor, action, host, detail…"
            class="h-8 pl-8 text-xs"
            data-testid="audit-search"
            @update:model-value="q = String($event) || null"
          />
        </div>
        <SelectMenu :model-value="since ?? '7d'" :options="sinceOpts" prefix="Since" content-class="w-44" aria-label="Since" @update:model-value="since = String($event)" />
        <SelectMenu :model-value="actor ?? ''" :options="actorOpts" prefix="Who" content-class="w-40" aria-label="Actor" @update:model-value="actor = String($event) || null" />
        <SelectMenu :model-value="family ?? ''" :options="familyOpts" prefix="What" content-class="w-40" aria-label="Action" @update:model-value="family = String($event) || null" />
        <SelectMenu :model-value="host ?? ''" :options="hostOpts" prefix="Where" content-class="w-44" aria-label="Host" @update:model-value="host = String($event) || null" />
        <Segmented :model-value="result ?? ''" class="h-8" aria-label="Result" data-testid="audit-result" @update:model-value="onResult">
          <SegmentedItem value="" class="px-2.5 text-xs">All</SegmentedItem>
          <SegmentedItem v-for="r in RESULTS" :key="r" :value="r" class="px-2.5 text-xs">{{ RESULT_META[r].label }}</SegmentedItem>
        </Segmented>
      </div>

      <p class="mb-2 text-xs text-muted-foreground" data-testid="audit-summary">
        <template v-if="loading">Loading…</template>
        <template v-else>
          {{ formatNumber(total) }} {{ total === 1 ? 'entry' : 'entries' }}
          <template v-if="entries.length < total"> · showing the newest {{ formatNumber(entries.length) }}</template>
          <template v-if="counts.failed"> · <span class="text-sev-error">{{ counts.failed }} failed</span></template>
          <template v-if="counts.denied"> · <span class="text-sev-warn">{{ counts.denied }} denied</span></template>
          <template v-if="counts.pending"> · {{ counts.pending }} pending</template>
        </template>
      </p>

      <DeployUnavailable v-if="unavailable" what="The audit log" />
      <div v-else-if="loading && !entries.length" class="flex flex-1 items-center justify-center py-16">
        <Spinner size="lg">Loading audit log…</Spinner>
      </div>
      <AuditTable v-else :entries="entries" :loading="loading" />
    </section>
  </AppShell>
</template>
