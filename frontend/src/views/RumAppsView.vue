<script setup>
// RUM executive summary (`/rum`): an at-a-glance overview of ALL frontend apps rather than a grid of
// per-app cards. It fans out one vitals / errors / pages query per registered app (`rumQueries.js`),
// zips them into a `perApp` array, and `lib/rumSummary` rolls that up into: a fleet health verdict,
// a KPI strip, a fleet-wide Core Web Vitals band (reusing WebVitalScorecard), a ranked apps table,
// a cross-app "live issues" feed, and the slowest routes. Time comes from the global context
// (ContextBar in AppShell); every drill-through rides `correlate()` so the window + scope tag along.
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import WebVitalScorecard from '@/components/rum/WebVitalScorecard.vue'
import RumFleetKpis from '@/components/rum/RumFleetKpis.vue'
import RumAppsTable from '@/components/rum/RumAppsTable.vue'
import RumIssuesFeed from '@/components/rum/RumIssuesFeed.vue'
import RumSlowestRoutes from '@/components/rum/RumSlowestRoutes.vue'
import RumManageAppsDialog from '@/components/rum/RumManageAppsDialog.vue'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { api } from '@/lib/core/api'
import { formatNumber, formatCompact } from '@/lib/core/format'
import { startNs, endNs } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import { useRumApps, useRumAppsVitals, useRumAppsErrors, useRumAppsPages } from '@/lib/rum/rumQueries'
import { fleetKpis, fleetVitals, rankApps, topIssues, slowestRoutes, formatVital, VITAL_FULL } from '@/lib/rum/rumSummary'

const router = useRouter()

const appsQuery = useRumApps()
// Enriched records (for the manage dialog) vs. plain names (for the existing per-app fan-out).
const appRecords = computed(() => appsQuery.data.value?.apps ?? [])
const apps = computed(() => appRecords.value.map((a) => a.name))
const manageOpen = ref(false)

// One query per app for each signal; results are index-aligned to `apps`.
const vitalsResults = useRumAppsVitals(apps, startNs, endNs)
const errorsResults = useRumAppsErrors(apps, startNs, endNs)
const pagesResults = useRumAppsPages(apps, startNs, endNs)

const perApp = computed(() =>
  apps.value.map((app, i) => ({
    app,
    vitals: vitalsResults.value?.[i]?.data?.vitals ?? [],
    errors: errorsResults.value?.[i]?.data?.errors ?? [],
    pages: pagesResults.value?.[i]?.data?.pages ?? [],
  })),
)

const appsLoading = computed(() => appsQuery.isLoading.value)

// --- Rollups (pure helpers) ---
const kpiData = computed(() => fleetKpis(perApp.value))
const bandVitals = computed(() => fleetVitals(perApp.value))
const appRows = computed(() => rankApps(perApp.value))
const issues = computed(() => topIssues(perApp.value, 6))
const routes = computed(() => slowestRoutes(perApp.value, 5))
const overallDist = computed(() =>
  bandVitals.value.reduce(
    (acc, v) => ({ good: acc.good + v.dist.good, needs: acc.needs + v.dist.needs, poor: acc.poor + v.dist.poor }),
    { good: 0, needs: 0, poor: 0 },
  ),
)

// App with the most JS errors — the "JS errors" KPI drills here.
const worstErrApp = computed(() => {
  let best = null
  for (const a of perApp.value) {
    const c = a.errors.reduce((s, e) => s + (e.count || 0), 0)
    if (c > 0 && (!best || c > best.c)) best = { app: a.app, c }
  }
  return best?.app ?? null
})

const kpis = computed(() => {
  const k = kpiData.value
  const gs = k.goodShare
  const p = k.appsPassing
  return [
    { key: 'pv', label: 'Pageviews', value: formatCompact(k.pageviews), accent: 'info' },
    {
      key: 'cwv',
      label: 'Core Web Vitals · good',
      value: gs == null ? '—' : Math.round(gs * 100) + '%',
      accent: gs == null ? 'neutral' : gs >= 0.75 ? 'success' : gs >= 0.5 ? 'warning' : 'error',
      dist: overallDist.value,
      sub: 'good · needs · poor',
    },
    {
      key: 'apps',
      label: 'Apps passing CWV',
      value: `${p.passing}/${p.total}`,
      accent: p.total === 0 ? 'neutral' : p.passing === p.total ? 'success' : p.passing === 0 ? 'error' : 'warning',
      sub: p.passing === p.total ? 'all healthy' : `${p.total - p.passing} need work`,
    },
    {
      key: 'err',
      label: 'JS errors',
      value: formatNumber(k.errors),
      accent: k.errors > 0 ? 'error' : 'success',
      sub: `${formatNumber(k.sessions)} sessions affected`,
      to: worstErrApp.value ? `/rum/${encodeURIComponent(worstErrApp.value)}/errors` : undefined,
    },
    {
      key: 'slow',
      label: 'Slowest app · LCP',
      value: k.slowestApp ? formatVital('web_vitals.lcp', k.slowestApp.p75) : '—',
      valueTone: k.slowestApp?.rating ?? null,
      accent: k.slowestApp
        ? k.slowestApp.rating === 'poor'
          ? 'error'
          : k.slowestApp.rating === 'needs'
            ? 'warning'
            : 'success'
        : 'neutral',
      sub: k.slowestApp?.app,
      to: k.slowestApp ? `/rum/${encodeURIComponent(k.slowestApp.app)}` : undefined,
    },
  ]
})

// CLS is unit-less; the time metrics carry an "ms" threshold suffix.
const unitFor = (metric) => (metric === 'web_vitals.cls' ? '' : 'ms')

// --- Navigation (every hop carries the window + scope) ---
function go(path) {
  router.push(correlate({ path }))
}
function openApp(app) {
  go(`/rum/${encodeURIComponent(app)}`)
}
function openErrors(app) {
  go(`/rum/${encodeURIComponent(app)}/errors`)
}
function openRoute({ app, route }) {
  go(`/rum/${encodeURIComponent(app)}/pages/${encodeURIComponent(route)}`)
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Frontend">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="rum-summary">

      <!-- First load -->
      <div v-if="appsLoading && !apps.length" class="flex flex-1 items-center justify-center">
        <Spinner size="lg">Loading frontend apps…</Spinner>
      </div>

      <!-- No apps: offer to register the first one (self-serve, no TOML). -->
      <div v-else-if="!apps.length" class="flex flex-1 flex-col items-center justify-center gap-4">
        <EmptyState
          title="No RUM apps"
          description="Instrument a web app with the Photon RUM SDK to see Core Web Vitals here."
          class="h-auto"
        />
        <button
          type="button"
          class="rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90"
          @click="manageOpen = true"
        >
          Add your first app
        </button>
      </div>

      <template v-else>
        <div class="flex items-center justify-end">
          <button
            type="button"
            class="rounded-md border border-border px-2.5 py-1 text-xs font-medium text-muted-foreground hover:bg-accent hover:text-foreground"
            @click="manageOpen = true"
          >
            Manage apps
          </button>
        </div>

        <!-- Fleet KPI strip -->
        <RumFleetKpis :kpis="kpis" @navigate="go" />

        <!-- Core Web Vitals band -->
        <section>
          <p class="mb-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
            Core Web Vitals · fleet-wide p75
          </p>
          <div v-if="bandVitals.length" class="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <WebVitalScorecard
              v-for="v in bandVitals"
              :key="v.metric"
              :metric="v.metric"
              :label="VITAL_FULL[v.metric] ?? v.metric"
              :p75="v.p75"
              :unit="unitFor(v.metric)"
              :rating="v.rating"
              :good-max="v.good_max"
              :poor-min="v.poor_min"
              :dist="v.dist"
            />
          </div>
          <p v-else class="rounded-lg border border-border bg-card px-4 py-6 text-center text-xs text-muted-foreground">
            No Core Web Vitals reported in this range.
          </p>
        </section>

        <!-- Apps table + live issues / slowest routes -->
        <section class="grid grid-cols-1 gap-3 lg:grid-cols-3">
          <div class="flex flex-col rounded-lg border border-border bg-surface-1 p-3 lg:col-span-2">
            <header class="mb-1 flex items-center justify-between gap-2">
              <h3 class="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Apps · ranked by health</h3>
              <span class="font-mono text-[11px] tabular-nums text-muted-foreground">{{ formatNumber(appRows.length) }}</span>
            </header>
            <RumAppsTable :rows="appRows" @open="openApp" />
          </div>

          <div class="flex flex-col gap-3">
            <div class="flex flex-col rounded-lg border border-border bg-surface-1 p-3">
              <header class="mb-1 flex items-center justify-between gap-2">
                <h3 class="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Live issues</h3>
                <button
                  v-if="issues.length"
                  type="button"
                  class="text-[11px] text-primary hover:underline"
                  @click="openErrors(issues[0].app)"
                >
                  view all
                </button>
              </header>
              <RumIssuesFeed v-if="issues.length" :issues="issues" @open="openErrors" />
              <p v-else class="py-6 text-center text-xs text-muted-foreground">No JS errors in this range.</p>
            </div>

            <div class="flex flex-col rounded-lg border border-border bg-surface-1 p-3">
              <header class="mb-1 flex items-center gap-2">
                <h3 class="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Slowest routes · LCP</h3>
              </header>
              <RumSlowestRoutes v-if="routes.length" :routes="routes" @open="openRoute" />
              <p v-else class="py-6 text-center text-xs text-muted-foreground">No route data in this range.</p>
            </div>
          </div>
        </section>
      </template>

      <RumManageAppsDialog v-model:open="manageOpen" :apps="appRecords" />
    </main>
  </AppShell>
</template>
