<script setup>
// RUM per-page detail (`/rum/:appId/pages/:route`): route-scoped Core Web Vitals (LCP/INP/CLS,
// coloured by the standard CWV thresholds), a device breakdown, and the route-scoped error issues.
// The LCP attribution panel is a LATER task (F2) — a placeholder slot is left for it below.
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import RumBreakdownTable from '@/components/rum/RumBreakdownTable.vue'
import LcpAttributionBar from '@/components/rum/LcpAttributionBar.vue'
import ErrorIssueList from '@/components/rum/ErrorIssueList.vue'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { ArrowLeft } from 'lucide-vue-next'
import { api } from '@/lib/core/api'
import { formatNumber } from '@/lib/core/format'
import { startNs, endNs } from '@/lib/core/context'
import { useRumPageDetail } from '@/lib/rum/rumQueries'
import { cn } from '@/lib/core/utils'

const route = useRoute()
const router = useRouter()

// Vue Router decodes both params automatically; normalize the array form defensively.
const app = computed(() => {
  const a = route.params.appId
  return ((Array.isArray(a) ? a[0] : a) ?? '').trim()
})
const pageRoute = computed(() => {
  const r = route.params.route
  return ((Array.isArray(r) ? r[0] : r) ?? '').trim()
})
const appBase = computed(() => '/rum/' + encodeURIComponent(app.value))
const pagesPath = computed(() => ({ path: `${appBase.value}/pages`, query: route.query }))
// Keep the app-level crumb (the in-view header carries the per-page context; scope set by vitals).
const crumb = computed(() => 'Frontend › ' + app.value)

// Time window is global (lib/context via ContextBar); this view just reads startNs/endNs.

const detailQuery = useRumPageDetail(app, pageRoute, startNs, endNs)
const detail = computed(() => detailQuery.data.value ?? null)
const loading = computed(() => detailQuery.isLoading.value)

const breakdown = computed(() => detail.value?.breakdown ?? [])
const errors = computed(() => detail.value?.errors ?? [])

// Route-scoped vitals carry no distribution/threshold data, so colour them against the standard
// CWV thresholds (same values RumBreakdownTable hardcodes): LCP 2500/4000, INP 200/500, CLS 0.1/0.25.
const CWV = [
  { field: 'lcp_p75', metric: 'web_vitals.lcp', label: 'LCP', good: 2500, poor: 4000 },
  { field: 'inp_p75', metric: 'web_vitals.inp', label: 'INP', good: 200, poor: 500 },
  { field: 'cls_p75', metric: 'web_vitals.cls', label: 'CLS', good: 0.1, poor: 0.25 },
]

function ratingFor(value, good, poor) {
  if (value == null || !Number.isFinite(value)) return null
  if (value <= good) return 'good'
  if (value <= poor) return 'needs'
  return 'poor'
}

function formatVital(metric, value) {
  if (value == null || !Number.isFinite(value)) return '—'
  if (metric === 'web_vitals.cls') return value.toFixed(2)
  if (value >= 1000) return (value / 1000).toFixed(1) + 's'
  return Math.round(value) + 'ms'
}

const TONE = {
  good: 'text-success',
  needs: 'text-sev-warn',
  poor: 'text-sev-error',
}

const vitalCards = computed(() => {
  const v = detail.value?.vitals
  if (!v) return []
  return CWV.map((c) => ({
    label: c.label,
    value: formatVital(c.metric, v[c.field]),
    rating: ratingFor(v[c.field], c.good, c.poor),
  }))
})

const pageviews = computed(() => detail.value?.vitals?.pageviews ?? null)

// LCP attribution (Task F2): shown only when at least one sub-part average or the top element is
// present (an SDK without the opt-in attribution module emits none of them → panel hidden).
const lcpAttribution = computed(() => {
  const a = detail.value?.attribution?.lcp
  if (!a) return null
  const hasData =
    a.ttfb != null ||
    a.resource_load_delay != null ||
    a.resource_load_time != null ||
    a.element_render_delay != null ||
    a.element != null
  return hasData ? a : null
})
</script>

<template>
  <AppShell :mock="api.mock" :crumb="crumb">
    <!-- Back-to-pages button folds into the ContextBar's lead slot (no second bar). -->
    <template #lead>
      <RouterLink
        :to="pagesPath"
        data-testid="back-to-pages"
        aria-label="Back to pages"
        class="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
      >
        <ArrowLeft class="size-4" />
      </RouterLink>
    </template>

    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div v-if="loading && !detail" class="flex flex-1 items-center justify-center">
        <Spinner size="lg">Loading page…</Spinner>
      </div>

      <template v-else>
        <!-- Route-scoped vitals -->
        <section class="px-5 pt-5">
          <div class="flex items-center gap-2.5 pb-3 text-xs text-muted-foreground">
            <span class="font-mono text-foreground">{{ pageRoute }}</span>
            <template v-if="pageviews != null">
              <span class="text-border">·</span>
              <span class="font-mono tabular-nums">{{ formatNumber(pageviews) }} pageviews</span>
            </template>
          </div>

          <div v-if="vitalCards.length" class="grid grid-cols-1 gap-4 sm:grid-cols-3">
            <div v-for="c in vitalCards" :key="c.label" class="rounded-xl border border-border bg-card p-4">
              <p class="text-xs text-muted-foreground">{{ c.label }}</p>
              <p :class="cn('mt-2 text-2xl font-semibold tabular-nums', TONE[c.rating] ?? 'text-foreground')">{{ c.value }}</p>
            </div>
          </div>
          <EmptyState
            v-else
            title="No web-vitals data for this route"
            description="Widen the time range to see LCP / INP / CLS for this page."
            class="h-auto py-10"
          />
        </section>

        <!-- LCP attribution (Task F2): why is LCP slow on this page? -->
        <section v-if="lcpAttribution" class="px-5 pt-6">
          <span class="pb-3 block text-xs font-medium uppercase tracking-wider text-muted-foreground">LCP Attribution</span>
          <LcpAttributionBar
            :ttfb="lcpAttribution.ttfb"
            :resource-load-delay="lcpAttribution.resource_load_delay"
            :resource-load-time="lcpAttribution.resource_load_time"
            :element-render-delay="lcpAttribution.element_render_delay"
            :element="lcpAttribution.element"
          />
        </section>

        <!-- Device breakdown -->
        <section class="flex min-h-0 flex-col px-5 pt-6">
          <span class="pb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground">Breakdown by Device</span>
          <RumBreakdownTable :rows="breakdown" key-label="Device" />
        </section>

        <!-- Route-scoped errors -->
        <section class="px-5 pb-5 pt-6">
          <ErrorIssueList :issues="errors" :service="app" />
        </section>
      </template>
    </main>
  </AppShell>
</template>
