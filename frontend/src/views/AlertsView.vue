<script setup lang="ts">
// Alerts (`/alerts`, Manage group): the webhook-alert engine — rules that watch metrics, logs,
// traces & RUM and notify channels when a condition is met. Own page header (like /uptime's <h1> +
// subtitle) rather than folding into the ContextBar's toolbar slot, a stat band, and 3 URL-synced
// tabs (`?tab=rules|incidents|channels`) each rendering a list/grid component. Tab sync copies
// DataView.vue's `?tab=` pattern (route.query.tab + query-preserving NavTabItem RouterLinks) rather
// than lib/core/useUrlState.ts, which only owns svc/sev/q/range and has no `tab` key. This task
// (T11) only stands up the shell — AlertStatBand/AlertRulesTable/IncidentsTable/ChannelsGrid, plus
// AlertRuleRow/AlertRuleDialog/ConditionBuilder/ChannelCard/ChannelDialog used by them, are empty
// stubs wired up in T13-T16. Terminology: OK · Pending · Triggered · Resolved — never "firing".
import { computed, ref } from 'vue'
import { useRoute } from 'vue-router'
import AppShell from '@/components/common/AppShell.vue'
import { NavTabs, NavTabItem } from '@/components/ui/nav-tabs'
import { api, type AlertRule } from '@/lib/core/api'
import AlertStatBand from '@/components/alerts/AlertStatBand.vue'
import AlertRulesTable from '@/components/alerts/AlertRulesTable.vue'
import AlertRuleDialog from '@/components/alerts/AlertRuleDialog.vue'
import IncidentsTable from '@/components/alerts/IncidentsTable.vue'
import ChannelsGrid from '@/components/alerts/ChannelsGrid.vue'

const route = useRoute()

const TABS = ['rules', 'incidents', 'channels'] as const
type AlertsTab = (typeof TABS)[number]
const tab = computed<AlertsTab>(() =>
  (TABS as readonly string[]).includes(route.query.tab as string)
    ? (route.query.tab as AlertsTab)
    : 'rules',
)

// The create/edit rule dialog: `editingRule` is null for "New alert" and the clicked row for
// "Edit" — AlertRuleDialog (T14) owns the create/update mutations itself, this view only tracks
// which rule (if any) it's open for.
const dialogOpen = ref(false)
const editingRule = ref<AlertRule | null>(null)

function openCreate() {
  editingRule.value = null
  dialogOpen.value = true
}
function openEdit(rule: AlertRule) {
  editingRule.value = rule
  dialogOpen.value = true
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Alerts">
    <section class="flex min-h-0 flex-1 flex-col overflow-y-auto px-5 pt-5 pb-6">
      <header class="mb-5 flex items-start justify-between gap-4">
        <div>
          <h1 class="text-xl font-semibold text-foreground">Alerts</h1>
          <p class="text-sm text-muted-foreground">
            Rules that watch metrics, logs, traces &amp; RUM — send a webhook when a condition is met.
          </p>
        </div>
      </header>

      <AlertStatBand class="mb-5" />

      <NavTabs class="mb-4">
        <NavTabItem
          :to="{ query: { ...route.query, tab: 'rules' } }"
          :active="tab === 'rules'"
          data-testid="alerts-tab-rules"
        >
          Rules
        </NavTabItem>
        <NavTabItem
          :to="{ query: { ...route.query, tab: 'incidents' } }"
          :active="tab === 'incidents'"
          data-testid="alerts-tab-incidents"
        >
          Incidents
        </NavTabItem>
        <NavTabItem
          :to="{ query: { ...route.query, tab: 'channels' } }"
          :active="tab === 'channels'"
          data-testid="alerts-tab-channels"
        >
          Channels
        </NavTabItem>
      </NavTabs>

      <AlertRulesTable v-if="tab === 'rules'" @open-create="openCreate" @edit="openEdit" />
      <IncidentsTable v-else-if="tab === 'incidents'" />
      <ChannelsGrid v-else-if="tab === 'channels'" />

      <AlertRuleDialog :open="dialogOpen" :rule="editingRule" @update:open="dialogOpen = $event" />
    </section>
  </AppShell>
</template>
