<script setup lang="ts">
// The service detail's sub-nav. Route sub-navigation, so this is the **nav-tabs** primitive —
// not Segmented (a toggle over one panel) and not Reka Tabs (in-page panels).
//
// Each tab is its own route with its own queries. That is the point of the split: Overview stays
// fast even when Operations is expensive, instead of one view fetching everything up front and the
// slowest panel setting the page's speed.
//
// `route.query` rides along on every link so the active window, Site and environment survive a tab
// change — a tab that resets the time range is a tab that loses your place.
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'

const props = defineProps<{ service: string }>()

const route = useRoute()
const base = computed(() => `/services/${encodeURIComponent(props.service)}`)

const TABS = [
  { key: 'overview', label: 'Overview', suffix: '' },
  { key: 'operations', label: 'Operations', suffix: '/operations' },
  { key: 'issues', label: 'Issues', suffix: '/errors' },
  { key: 'dependencies', label: 'Dependencies', suffix: '/dependencies' },
  { key: 'deployments', label: 'Deployments', suffix: '/deployments' },
] as const

// A tab is active when the path is its own or one of its children (`/operations/GET+%2Fa` keeps
// Operations lit). Overview is the bare base, so it matches exactly or nothing would ever be
// unlit.
function isActive(suffix: string): boolean {
  const path = route.path.replace(/\/$/, '')
  if (!suffix) return path === base.value
  return path === base.value + suffix || path.startsWith(base.value + suffix + '/')
}
</script>

<template>
  <NavTabs class="text-xs" aria-label="Service sections">
    <NavTabItem
      v-for="t in TABS"
      :key="t.key"
      :data-tab="t.key"
      :to="{ path: base + t.suffix, query: route.query }"
      :active="isActive(t.suffix)"
    >
      {{ t.label }}
    </NavTabItem>
  </NavTabs>
</template>
