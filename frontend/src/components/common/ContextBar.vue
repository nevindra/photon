<script setup lang="ts">
// App-wide context bar (mounted once in AppShell). Owns the global time window, the tenant (Site)
// picker, and the environment picker. THE single header row for every view — no view renders a
// second bar. Layout,
// left → right: an optional `lead` slot (e.g. a back button), the breadcrumb (+ the tenant and
// environment pickers, each of which hides when it has nothing to choose between), an
// optional search region in the middle (the `search` slot — searchable views forward their
// SearchBar here via AppShell's `toolbar` slot), then the right cluster: an optional `actions` slot
// (view-specific controls), the optional LiveControl, and the time picker. When the middle `search`
// slot is empty its `flex-1` container is just the spacer that pushes the right cluster over; the
// `lead`/`actions` slots render nothing when empty (flexbox adds no gap around an absent child).
// Bound to lib/context.ts (module-singleton state, NOT Pinia).
import { computed, watch } from 'vue'
import { ChevronDown, Building2, Layers } from 'lucide-vue-next'
import TimeRangePicker from '@/components/common/TimeRangePicker.vue'
import LiveControl from '@/components/common/LiveControl.vue'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
} from '@/components/ui/dropdown-menu'
import { useTenants } from '@/lib/tenants/tenantsQueries'
import { useEnvironments } from '@/lib/core/environments'
import {
  timeRange,
  customRange,
  tenant,
  environment,
  startNs,
  endNs,
  setTimeRange,
  setCustomRange,
  setTenant,
  clearTenant,
  setEnvironment,
  clearEnvironment,
  applyDefaultEnvironment,
} from '@/lib/core/context'

interface Props {
  /** Leading breadcrumb text, e.g. 'Home · Overview' or 'Backend'. */
  crumb?: string
  /** Forwarded to LiveControl. */
  liveMode?: string
  liveStatus?: string
  /** Show the LiveControl at all. */
  live?: boolean
  /** Show the global time picker. `/home` turns it off: the Attention Board judges over its own
   *  bounded Watch Range, and two time controls in one header would be two sources of truth. */
  timePicker?: boolean
}

withDefaults(defineProps<Props>(), {
  crumb: '',
  liveMode: 'manual',
  liveStatus: 'idle',
  live: false,
  timePicker: true,
})

defineEmits<{
  (e: 'update:liveMode', mode: string): void
  (e: 'refresh'): void
}>()

// The tenant picker only renders on installs that actually federate (registry non-empty) —
// a plain single-tenant photon never sees it.
const tenantsQuery = useTenants()
const tenantNames = computed<string[]>(
  () => tenantsQuery.data.value?.tenants?.map((t) => t.name) ?? [],
)
function onTenantPick(v: unknown): void {
  typeof v === 'string' && v ? setTenant(v) : clearTenant()
}

// The environment picker renders only when there is a choice to make: two or more
// `deployment.environment` values in the window. An install whose producers declare none, or all
// declare the same one, gets no control — the same rule that hides the tenant picker on an empty
// registry. A picker over a constant is a control that cannot change anything.
const environmentsQuery = useEnvironments(startNs, endNs)
const environmentNames = computed<string[]>(() => environmentsQuery.data.value ?? [])
const showEnvironments = computed<boolean>(() => environmentNames.value.length >= 2)

// First facet result that offers a real choice picks the default (`prod` when it exists) — see
// applyDefaultEnvironment for why prod and why only once.
watch(environmentNames, applyDefaultEnvironment, { immediate: true })

function onEnvironmentPick(v: unknown): void {
  typeof v === 'string' && v ? setEnvironment(v) : clearEnvironment()
}
</script>

<template>
  <header class="flex h-12 w-full items-center gap-3 border-b border-border bg-surface-1 px-4">
    <!-- Optional lead region (e.g. a back button), far left before the crumb. Empty → renders
         nothing, so flexbox adds no leading gap. -->
    <slot name="lead" />

    <span class="shrink-0 truncate text-sm font-semibold text-foreground">{{ crumb }}</span>

    <DropdownMenu v-if="tenantNames.length">
      <DropdownMenuTrigger
        data-testid="tenant-picker"
        class="inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs"
        :class="tenant
          ? 'border-brand/35 bg-brand-soft text-brand'
          : 'border-border text-muted-foreground hover:text-foreground'"
      >
        <Building2 class="size-3" />
        {{ tenant ?? 'All tenants' }}
        <ChevronDown class="size-3 opacity-70" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuRadioGroup :model-value="tenant ?? ''" @update:model-value="onTenantPick">
          <DropdownMenuRadioItem value="">All tenants</DropdownMenuRadioItem>
          <DropdownMenuRadioItem v-for="name in tenantNames" :key="name" :value="name">
            {{ name }}
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>

    <DropdownMenu v-if="showEnvironments">
      <DropdownMenuTrigger
        data-testid="env-picker"
        class="inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs"
        :class="environment
          ? 'border-brand/35 bg-brand-soft text-brand'
          : 'border-border text-muted-foreground hover:text-foreground'"
      >
        <Layers class="size-3" />
        {{ environment ?? 'All environments' }}
        <ChevronDown class="size-3 opacity-70" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuRadioGroup :model-value="environment ?? ''" @update:model-value="onEnvironmentPick">
          <DropdownMenuRadioItem value="">All environments</DropdownMenuRadioItem>
          <DropdownMenuRadioItem v-for="name in environmentNames" :key="name" :value="name">
            {{ name }}
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>

    <!-- Middle search region. Empty for non-searchable views → an inert flex-1 spacer that
         pushes the time cluster to the right (replaces the old `ml-auto`). -->
    <div class="min-w-0 flex-1">
      <slot name="search" />
    </div>

    <div class="flex shrink-0 items-center gap-3">
      <!-- View-specific action controls, before the live/time cluster. Empty → renders nothing. -->
      <slot name="actions" />
      <LiveControl
        v-if="live"
        :mode="liveMode"
        :status="liveStatus"
        @update:mode="$emit('update:liveMode', $event)"
        @refresh="$emit('refresh')"
      />
      <TimeRangePicker
        v-if="timePicker"
        :model-value="timeRange"
        :custom-range="customRange ?? undefined"
        @update:model-value="setTimeRange"
        @update:custom-range="setCustomRange"
      />
    </div>
  </header>
</template>
