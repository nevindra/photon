<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import { Settings, LogOut } from 'lucide-vue-next'
import PhotonMark from '@/components/common/PhotonMark.vue'
import ThemeToggle from '@/components/common/ThemeToggle.vue'
import {
  HomeIcon,
  FrontendIcon,
  BackendIcon,
  HostsIcon,
  UptimeIcon,
  LogsIcon,
  TracesIcon,
  MetricsIcon,
  TemplatesIcon,
  StacksIcon,
  RolloutsIcon,
  AlertsIcon,
  DataIcon,
  TenantsIcon,
  AuditIcon,
} from '@/components/common/nav-icons'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuItem,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/core/utils'
import { username } from '@/lib/core/auth'

defineProps({
  active: { type: String, default: 'logs' },
  mock: { type: Boolean, default: false },
})
const emit = defineEmits(['select', 'logout'])

// The group column scrolls on a short viewport (the rail carries more items than a 900px screen
// holds). A hidden scrollbar hides the fact, so a bottom fade says "more below" only while the
// column actually overflows — a fade on a column that fits would just dim the last item.
const column = ref(null)
const overflowing = ref(false)
let ro = null
function measure() {
  const el = column.value
  overflowing.value = !!el && el.scrollHeight > el.clientHeight + 1
}
onMounted(() => {
  measure()
  if (typeof ResizeObserver !== 'undefined' && column.value) {
    ro = new ResizeObserver(measure)
    ro.observe(column.value)
  }
})
onBeforeUnmount(() => ro?.disconnect())
// Two-letter avatar initials from the signed-in username (falls back to '?').
const initials = computed(() => (username.value || '?').slice(0, 2).toUpperCase())

// The rail is grouped by what the operator is trying to DO — four verbs under an ungrouped Home:
//
//   Monitor — the health of what you run, outside-in: Frontend (your users' browsers), Backend
//             (your services), Hosts (the machines), Uptime (the probes watching from outside).
//   Explore — the raw signals underneath those judgments: Logs, Traces, Metrics.
//   Deploy  — changing what runs: Templates (recipes), Stacks (what is applied), Rollouts
//             (applying it in stages).
//   Manage  — Photon itself: Alerts (the rules and channels), Data (storage + retention),
//             Tenants (the federation registry), Audit (what Photon changed on your hosts).
//
// The earlier layout split Monitor into three fragments (a hidden "Frontend" heading, a hidden
// "Backend" heading, an "Infra" heading over Hosts + "Ops") — seven groups for sixteen items, with
// "Ops" an opaque name for the uptime page. Four headings, every one a verb, is the whole idea.
//
// `route` is the landing path AppShell pushes to on select (see AppShell.vue's LANDING map); `key`
// is what's compared against `active` and stamped as `data-nav` for tests/E2E hooks. Headings must
// fit the 74px rail: ~66px of content at 9px + tracking-wider is about 10 characters ("Monitor",
// "Explore", "Deploy", "Manage" all do; "Infrastructure" once bled past both edges).
const NAV_GROUPS = [
  { items: [{ key: 'home', label: 'Home', icon: HomeIcon, route: '/home' }] },
  {
    label: 'Monitor',
    items: [
      { key: 'frontend', label: 'Frontend', icon: FrontendIcon, route: '/rum' },
      { key: 'backend', label: 'Backend', icon: BackendIcon, route: '/services' },
      { key: 'infra', label: 'Hosts', icon: HostsIcon, route: '/infra' },
      { key: 'uptime', label: 'Uptime', icon: UptimeIcon, route: '/uptime' },
    ],
  },
  {
    label: 'Explore',
    items: [
      { key: 'logs', label: 'Logs', icon: LogsIcon, route: '/logs' },
      { key: 'traces', label: 'Traces', icon: TracesIcon, route: '/traces' },
      { key: 'metrics', label: 'Metrics', icon: MetricsIcon, route: '/metrics' },
    ],
  },
  {
    label: 'Deploy',
    items: [
      { key: 'templates', label: 'Templates', icon: TemplatesIcon, route: '/deploy/templates' },
      { key: 'stacks', label: 'Stacks', icon: StacksIcon, route: '/deploy/stacks' },
      { key: 'rollouts', label: 'Rollouts', icon: RolloutsIcon, route: '/deploy/rollouts' },
    ],
  },
  {
    label: 'Manage',
    items: [
      { key: 'alerts', label: 'Alerts', icon: AlertsIcon, route: '/alerts' },
      { key: 'data', label: 'Data', icon: DataIcon, route: '/data' },
      { key: 'tenants', label: 'Tenants', icon: TenantsIcon, route: '/tenants' },
      { key: 'audit', label: 'Audit', icon: AuditIcon, route: '/audit' },
    ],
  },
]

function onSelect(item) {
  emit('select', item.key)
}
</script>

<template>
  <nav
    class="flex h-full w-[74px] shrink-0 flex-col items-center gap-1 border-r border-border bg-surface-1 py-3"
  >
    <div class="relative">
      <PhotonMark :size="30" />
      <span
        class="absolute -bottom-0.5 -right-0.5 size-2 rounded-full bg-brand ring-2 ring-background"
        aria-hidden="true"
      />
    </div>

    <!-- The group column scrolls (scrollbar hidden) rather than pushing the account cluster off a
         short viewport now that the rail carries more than a dozen items. -->
    <div
      ref="column"
      data-testid="nav-column"
      :class="cn('mt-4 flex min-h-0 w-full flex-col items-center gap-0.5 overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden', overflowing && '[mask-image:linear-gradient(to_bottom,black_calc(100%-28px),transparent)]')"
      @scroll="measure"
    >
      <template v-for="group in NAV_GROUPS" :key="group.label ?? 'root'">
        <!-- A heading with a hairline above it: the line, not just the whitespace, is what makes
             four groups read as four groups in a column this narrow. -->
        <div
          v-if="group.label"
          data-nav-group
          class="mb-0.5 mt-1.5 w-full truncate border-t border-border/70 px-1 pt-1.5 text-center text-[9px] font-semibold uppercase tracking-wider text-muted-foreground/60"
        >
          {{ group.label }}
        </div>

        <!-- Two-tone icons: the neutral strokes follow the button's text colour (muted when idle,
             ink when active) and the cyan accent is set through --ni-accent by the `nav-item`
             rules below — softened while idle so fifteen accents stay a rail and not a rainbow,
             full strength on hover and on the active item. -->
        <button
          v-for="item in group.items"
          :key="item.key"
          type="button"
          :data-nav="item.key"
          :class="
            cn(
              'nav-item relative flex w-[58px] flex-col items-center gap-[3px] rounded-lg py-1 transition-colors',
              item.key === active
                ? 'is-active bg-brand-soft text-brand'
                : 'text-muted-foreground hover:bg-muted hover:text-foreground',
            )
          "
          @click="onSelect(item)"
        >
          <span
            v-if="item.key === active"
            class="absolute -left-2 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r-full bg-brand"
          />
          <component
            :is="item.icon"
            :class="cn('size-5', item.key === active && 'text-foreground')"
          />
          <span class="text-[10px] font-medium leading-none">{{ item.label }}</span>
        </button>
      </template>
    </div>

    <div class="mt-auto flex flex-col items-center gap-1.5 pb-1 pt-2">
      <Tooltip>
        <TooltipTrigger as-child>
          <span
            class="flex h-4 w-8 cursor-default items-center justify-center"
            :aria-label="mock ? 'demo data' : 'connected'"
          >
            <span
              data-testid="connected-dot"
              :class="cn('size-2 rounded-full', mock ? 'bg-amber-500' : 'bg-green-500')"
            />
          </span>
        </TooltipTrigger>
        <TooltipContent side="right">{{ mock ? 'demo data' : 'connected' }}</TooltipContent>
      </Tooltip>

      <!-- Settings and the theme toggle share a row: the rail is 74px wide, two 32px buttons fit,
           and the vertical space matters more than the symmetry. -->
      <div class="flex items-center gap-0.5">
        <button
          type="button"
          data-nav="settings"
          :class="
            cn(
              'flex size-8 items-center justify-center rounded-lg transition-colors',
              active === 'settings' ? 'bg-brand-soft text-brand' : 'text-muted-foreground hover:bg-muted hover:text-foreground',
            )
          "
          aria-label="Settings"
          @click="emit('select', 'settings')"
        >
          <Settings class="size-[18px]" />
        </button>
        <ThemeToggle />
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <button
            type="button"
            class="flex size-8 items-center justify-center rounded-full bg-primary text-[11px] font-semibold text-primary-foreground"
            aria-label="Account menu"
          >
            {{ initials }}
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="right" align="end" class="w-48">
          <DropdownMenuLabel class="font-normal">
            <div class="flex flex-col gap-0.5">
              <span class="text-sm font-medium leading-none">{{ username || 'Account' }}</span>
              <span class="text-xs text-muted-foreground">Signed in</span>
            </div>
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem @select="emit('logout')">
            <LogOut class="size-4" />
            Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>

  </nav>
</template>

<style scoped>
/* The accent tone of a two-tone nav icon (see nav-icons/NavIconBase.vue). Idle items carry the
   cyan softened; hover and the active item bring it to full strength. */
.nav-item {
  --ni-accent: hsl(var(--brand) / 0.7);
}
.nav-item:hover,
.nav-item.is-active {
  --ni-accent: hsl(var(--brand));
}
</style>
