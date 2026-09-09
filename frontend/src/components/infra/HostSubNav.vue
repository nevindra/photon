<script setup lang="ts">
// The host page's sub-nav — route-level sub-navigation, so the `nav-tabs` primitive (not
// Segmented, not Reka Tabs). Each tab is its own route with its own queries: Overview keeps
// painting fast while the Docker inventory (a live agent read) loads on its own tab.
// `route.query` rides along so the window, Site and environment survive a tab change.
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'

const props = defineProps<{ host: string }>()

const route = useRoute()
const base = computed(() => `/infra/${encodeURIComponent(props.host)}`)

export type HostTab = 'overview' | 'containers' | 'agent'
const TABS: { key: HostTab; label: string; suffix: string }[] = [
  { key: 'overview', label: 'Overview', suffix: '' },
  { key: 'containers', label: 'Containers', suffix: '/containers' },
  { key: 'agent', label: 'Agent', suffix: '/agent' },
]

function isActive(suffix: string): boolean {
  const path = route.path.replace(/\/$/, '')
  if (!suffix) return path === base.value
  return path === base.value + suffix || path.startsWith(base.value + suffix + '/')
}
</script>

<template>
  <NavTabs class="text-xs" aria-label="Host sections">
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
