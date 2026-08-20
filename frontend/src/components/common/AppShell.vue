<script setup>
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import NavRail from '@/components/common/NavRail.vue'
import ContextBar from '@/components/common/ContextBar.vue'
import { logout } from '@/lib/core/auth'

const props = defineProps({
  // Optional override (tests / edge cases); normally derived from the route below.
  active: { type: String, default: null },
  mock: { type: Boolean, default: false },
  // Forwarded to the global ContextBar (Task 5): breadcrumb + optional live-tail control.
  crumb: { type: String, default: '' },
  live: { type: Boolean, default: false },
  liveMode: { type: String, default: 'manual' },
  liveStatus: { type: String, default: 'idle' },
  // What the view's content sits on. 'canvas' (default) is the grey app canvas — right for
  // dashboards, where every block is its own white card and the grey is what makes the cards
  // read as cards. 'sheet' is one white working surface edge-to-edge — right for the dense
  // explorers (logs / traces / metrics), whose tables are full-bleed and own no panel of their
  // own; on the canvas they'd be a grey table. A view that puts every table in a panel should
  // stay on 'canvas'.
  surface: { type: String, default: 'canvas' },
})
defineEmits(['update:liveMode', 'refresh'])

const route = useRoute()
const router = useRouter()

// NavRail's groups don't map 1:1 onto route segments — several sections (Frontend/Backend/
// Ops) are "worlds" fronted by an existing route (rum/services/uptime), and Home has
// no route segment of its own. ROUTE_GROUP translates the route's first path segment to the
// nav-group key that should be highlighted; LANDING is the inverse (nav-group key → the route
// NavRail selecting that group should push to).
const ROUTE_GROUP = {
  home: 'home',
  rum: 'frontend',
  services: 'backend',
  logs: 'logs',
  traces: 'traces',
  metrics: 'metrics',
  infra: 'infra',
  uptime: 'infrastructure',
  data: 'data',
  alerts: 'alerts',
  tenants: 'tenants',
}
const LANDING = {
  home: '/home',
  frontend: '/rum',
  backend: '/services',
  infra: '/infra',
  infrastructure: '/uptime',
  logs: '/logs',
  traces: '/traces',
  metrics: '/metrics',
  data: '/data',
  alerts: '/alerts',
  tenants: '/tenants',
}

// The first path segment is the route section: '/logs' → 'logs', '/traces/abc' → 'traces',
// resolved to a nav-group key via ROUTE_GROUP. An explicit `active` prop wins so tests can pin
// it; otherwise the route drives the highlighted group.
const active = computed(
  () => props.active ?? (ROUTE_GROUP[route.path.split('/')[1]] ?? 'home'),
)

function onSelect(key) {
  router.push(LANDING[key] ?? '/' + key)
}

async function onLogout() {
  await logout()
  router.push('/login')
}
</script>

<template>
  <div class="flex h-full min-h-0">
    <NavRail :active="active" :mock="mock" @select="onSelect" @logout="onLogout" />
    <div
      class="flex min-h-0 min-w-0 flex-1 flex-col"
      :class="surface === 'sheet' ? 'bg-surface-1' : 'bg-background'"
    >
      <ContextBar
        :crumb="crumb"
        :live="live"
        :live-mode="liveMode"
        :live-status="liveStatus"
        @update:live-mode="$emit('update:liveMode', $event)"
        @refresh="$emit('refresh')"
      >
        <!-- Views fold their per-view header chrome into the one ContextBar: an optional back
             button (`lead`), their SearchBar or sub-nav tabs (`toolbar` → the middle search
             region), and view-specific action controls (`actions`). Absent slots render nothing. -->
        <template #lead><slot name="lead" /></template>
        <template #search><slot name="toolbar" /></template>
        <template #actions><slot name="actions" /></template>
      </ContextBar>
      <slot />
    </div>
  </div>
</template>
