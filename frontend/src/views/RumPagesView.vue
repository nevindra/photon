<script setup>
// RUM Pages list (`/rum/:appId/pages`): per-route rollup table (pageviews + LCP/INP/CLS p75).
// Same shell + sub-nav as the vitals hero (Pages active). Each `/api/rum/pages` row carries a
// `route`; RumBreakdownTable expects a `key`, so map `key: row.route`. Row click → page detail.
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import RumBreakdownTable from '@/components/rum/RumBreakdownTable.vue'
import { Spinner } from '@/components/ui/spinner'
import { api } from '@/lib/core/api'
import { formatNumber } from '@/lib/core/format'
import { timeRange, customRange, startNs, endNs } from '@/lib/core/context'
import { useRumPages } from '@/lib/rum/rumQueries'

const route = useRoute()
const router = useRouter()

const app = computed(() => {
  const a = route.params.appId
  return ((Array.isArray(a) ? a[0] : a) ?? '').trim()
})
const appBase = computed(() => '/rum/' + encodeURIComponent(app.value))
// Keep the app-level crumb (this is a sub-page of the same RUM app; scope set by the vitals view).
const crumb = computed(() => 'Frontend › ' + app.value)

const pagesQuery = useRumPages(app, startNs, endNs)
const rows = computed(() => (pagesQuery.data.value?.pages ?? []).map((p) => ({ ...p, key: p.route })))
const loading = computed(() => pagesQuery.isFetching.value)

function onPageRow(row) {
  if (row?.key) router.push(`${appBase.value}/pages/${encodeURIComponent(row.key)}`)
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="crumb">
    <!-- Web Vitals · Pages · Errors sub-nav folds into the ContextBar's search region. -->
    <template #toolbar>
      <NavTabs class="text-xs">
        <NavTabItem :to="{ path: appBase, query: route.query }" :active="false">Web Vitals</NavTabItem>
        <NavTabItem :to="{ path: `${appBase}/pages`, query: route.query }" :active="true">Pages</NavTabItem>
        <NavTabItem :to="{ path: `${appBase}/errors`, query: route.query }" :active="false">Errors</NavTabItem>
      </NavTabs>
    </template>

    <main class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div class="flex items-center gap-2.5 px-5 pb-2 pt-5 text-xs text-muted-foreground">
        <span class="font-mono tabular-nums text-foreground/80">{{ formatNumber(rows.length) }} routes</span>
        <span class="text-border">·</span>
        <span class="font-mono">{{ customRange ? 'custom range' : `last ${timeRange}` }}</span>
        <Spinner v-if="loading" size="sm">loading…</Spinner>
      </div>

      <RumBreakdownTable :rows="rows" key-label="Route" :loading="loading" clickable @row-click="onPageRow" />
    </main>
  </AppShell>
</template>
