<script setup>
// RUM Web Vitals hero (`/rum/:appId`): a sub-nav (Web Vitals · Pages · Errors), a scorecard per
// returned Core Web Vital, and a dimension-switchable breakdown table. Clone of ServiceDetailView's
// route-param + time-range plumbing. Zero-sample vitals are OMITTED by the API, so we render only
// what comes back (never assume all five).
import { ref, computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import WebVitalScorecard from '@/components/rum/WebVitalScorecard.vue'
import RumBreakdownTable from '@/components/rum/RumBreakdownTable.vue'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { api } from '@/lib/core/api'
import { startNs, endNs, setScope } from '@/lib/core/context'
import { useRumVitals, useRumBreakdown } from '@/lib/rum/rumQueries'

const route = useRoute()
const router = useRouter()

// Vue Router decodes params automatically; normalize the array form (repeated param) defensively.
const app = computed(() => {
  const a = route.params.appId
  return ((Array.isArray(a) ? a[0] : a) ?? '').trim()
})
const appBase = computed(() => '/rum/' + encodeURIComponent(app.value))
// Breadcrumb: this app is the active entity scope. `immediate` seeds it on mount; the watch also
// keeps it current if Vue reuses this instance across an `:appId` change (same route record).
const crumb = computed(() => 'Frontend › ' + app.value)
watch(app, (a) => setScope({ type: 'rumApp', id: a, label: a }), { immediate: true })

// Time window is global (lib/context via ContextBar); this view just reads startNs/endNs.

const VITAL_LABELS = {
  'web_vitals.lcp': 'LCP',
  'web_vitals.inp': 'INP',
  'web_vitals.cls': 'CLS',
  'web_vitals.fcp': 'FCP',
  'web_vitals.ttfb': 'TTFB',
}

// --- Web Vitals scorecards ---
const vitalsQuery = useRumVitals(app, startNs, endNs)
const vitals = computed(() => vitalsQuery.data.value?.vitals ?? [])
const vitalsLoading = computed(() => vitalsQuery.isLoading.value)

// Map one API vitals entry (snake_case good_max/poor_min) → WebVitalScorecard props (camelCase).
function scorecardProps(v) {
  return {
    metric: v.metric,
    label: VITAL_LABELS[v.metric] ?? v.metric,
    p75: v.p75,
    unit: v.metric === 'web_vitals.cls' ? '' : 'ms',
    rating: v.rating,
    goodMax: v.good_max,
    poorMin: v.poor_min,
    dist: v.dist,
  }
}

// --- breakdown-by-dimension ---
const DIMENSIONS = [
  { value: 'browser.route', label: 'Route' },
  { value: 'device.type', label: 'Device' },
  { value: 'browser.name', label: 'Browser' },
  { value: 'geo.country', label: 'Country' },
  { value: 'network.connection', label: 'Connection' },
]
const dimension = ref('browser.route')
function setDimension(d) {
  if (d) dimension.value = d
}
const dimLabel = computed(() => DIMENSIONS.find((d) => d.value === dimension.value)?.label ?? 'Segment')
const isRouteDimension = computed(() => dimension.value === 'browser.route')

const breakdownQuery = useRumBreakdown(app, dimension, startNs, endNs)
const breakdownRows = computed(() => breakdownQuery.data.value?.rows ?? [])
const breakdownLoading = computed(() => breakdownQuery.isFetching.value)

// Route rows drill into the per-page detail; other dimensions are informational only.
function onBreakdownRow(row) {
  if (isRouteDimension.value && row?.key) {
    router.push(`${appBase.value}/pages/${encodeURIComponent(row.key)}`)
  }
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="crumb">
    <!-- Web Vitals · Pages · Errors sub-nav folds into the ContextBar's search region. -->
    <template #toolbar>
      <NavTabs class="text-xs">
        <NavTabItem :to="{ path: appBase, query: route.query }" :active="true">Web Vitals</NavTabItem>
        <NavTabItem :to="{ path: `${appBase}/pages`, query: route.query }" :active="false">Pages</NavTabItem>
        <NavTabItem :to="{ path: `${appBase}/errors`, query: route.query }" :active="false">Errors</NavTabItem>
      </NavTabs>
    </template>

    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <!-- Core Web Vitals scorecards -->
      <section class="px-5 pt-5">
        <div v-if="vitalsLoading && !vitals.length" class="flex items-center justify-center py-16">
          <Spinner size="lg">Loading web vitals…</Spinner>
        </div>
        <EmptyState
          v-else-if="!vitals.length"
          title="No web-vitals data in range"
          description="Widen the time range, or check that the RUM SDK is reporting."
          class="h-auto py-16"
        />
        <div v-else class="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-5">
          <WebVitalScorecard v-for="v in vitals" :key="v.metric" v-bind="scorecardProps(v)" />
        </div>
      </section>

      <!-- Breakdown by dimension -->
      <section class="flex min-h-0 flex-col px-5 pb-5 pt-6">
        <div class="flex items-center justify-between gap-2.5 pb-3">
          <span class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Breakdown by {{ dimLabel }}</span>
          <Segmented :model-value="dimension" @update:model-value="setDimension">
            <SegmentedItem v-for="d in DIMENSIONS" :key="d.value" :value="d.value">{{ d.label }}</SegmentedItem>
          </Segmented>
        </div>
        <RumBreakdownTable
          :rows="breakdownRows"
          :key-label="dimLabel"
          :loading="breakdownLoading"
          :clickable="isRouteDimension"
          @row-click="onBreakdownRow"
        />
      </section>
    </main>
  </AppShell>
</template>
