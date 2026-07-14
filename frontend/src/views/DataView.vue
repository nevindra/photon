<script setup>
// The `/data` page: a single-header-row shell (AppShell → the global ContextBar) hosting four
// URL-synced tabs — Overview (usage/footprint charts + tiles), Storage (per-signal cards + durable
// strip), Retention (the retention form), and Delete (per-signal purge actions). The active tab
// lives in the `?tab=` query param so tabs are deep-linkable and survive a refresh. The tab switch
// folds into the ContextBar's search region as a query-driven sub-nav (NavTabItem is a RouterLink),
// so there's no second header bar; the four bodies render conditionally by the active tab. Each
// body is its own component and owns its data composables, mirroring the settings dialog this page
// retired.
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { Activity, HardDrive, Timer, Trash2 } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import { api } from '@/lib/core/api'
import DataOverview from '@/components/data/DataOverview.vue'
import DataStorage from '@/components/data/DataStorage.vue'
import DataRetention from '@/components/data/DataRetention.vue'
import DataDelete from '@/components/data/DataDelete.vue'

const route = useRoute()

// Read-only now: navigation is via the NavTabItem RouterLinks (each writes `?tab=`), so the getter
// just resolves the active tab from the URL and falls back to Overview for a missing/unknown value.
const TABS = ['overview', 'storage', 'retention', 'delete']
const tab = computed(() => (TABS.includes(route.query.tab) ? route.query.tab : 'overview'))
</script>

<template>
  <AppShell :mock="api.mock" crumb="Data">
    <!-- Tab switcher folds into the ContextBar's search region — a query-driven sub-nav so tabs stay
         deep-linkable (?tab=) without a second header row (Reka <TabsList> is coupled to <Tabs>, so
         we mirror the other views' NavTabs pattern instead of relocating it). -->
    <template #toolbar>
      <NavTabs class="text-xs">
        <NavTabItem
          :to="{ query: { ...route.query, tab: 'overview' } }"
          :active="tab === 'overview'"
          data-testid="data-tab-overview"
        >
          <Activity /> Overview
        </NavTabItem>
        <NavTabItem
          :to="{ query: { ...route.query, tab: 'storage' } }"
          :active="tab === 'storage'"
          data-testid="data-tab-storage"
        >
          <HardDrive /> Storage
        </NavTabItem>
        <NavTabItem
          :to="{ query: { ...route.query, tab: 'retention' } }"
          :active="tab === 'retention'"
          data-testid="data-tab-retention"
        >
          <Timer /> Retention
        </NavTabItem>
        <NavTabItem
          :to="{ query: { ...route.query, tab: 'delete' } }"
          :active="tab === 'delete'"
          data-testid="data-tab-delete"
        >
          <Trash2 /> Delete
        </NavTabItem>
      </NavTabs>
    </template>

    <section class="flex min-h-0 flex-1 flex-col overflow-y-auto px-5 pt-5 pb-6">
      <DataOverview v-if="tab === 'overview'" />
      <DataStorage v-else-if="tab === 'storage'" />
      <DataRetention v-else-if="tab === 'retention'" />
      <DataDelete v-else-if="tab === 'delete'" />
    </section>
  </AppShell>
</template>
