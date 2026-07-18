<script setup>
import { ref, computed } from 'vue'
import {
  Home,
  MonitorSmartphone,
  Gauge,
  Activity,
  Server,
  ScrollText,
  Waypoints,
  BarChart3,
  Database,
  Bell,
  Settings,
  LogOut,
} from 'lucide-vue-next'
import PhotonMark from '@/components/common/PhotonMark.vue'
import ThemeToggle from '@/components/common/ThemeToggle.vue'
import SettingsDialog from '@/components/common/SettingsDialog.vue'
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

const settingsOpen = ref(false)
// Two-letter avatar initials from the signed-in username (falls back to '?').
const initials = computed(() => (username.value || '?').slice(0, 2).toUpperCase())

// Nav is grouped into ownership "worlds" (Frontend / Backend / Ops — each a single
// landing item today, room to grow into their own sub-nav later), an Explore section for the raw
// signal browsers (Logs/Traces/Metrics), and Manage for cross-cutting admin (Data). Home sits
// ungrouped at the top. `route` is the landing path AppShell pushes to on select (see
// AppShell.vue's LANDING map); `key` is what's compared against `active` and stamped as
// `data-nav` for tests/E2E hooks.
const NAV_GROUPS = [
  { items: [{ key: 'home', label: 'Home', icon: Home, route: '/home' }] },
  {
    label: 'Frontend',
    items: [{ key: 'frontend', label: 'Frontend', icon: MonitorSmartphone, route: '/rum' }],
  },
  {
    label: 'Backend',
    items: [{ key: 'backend', label: 'Backend', icon: Gauge, route: '/services' }],
  },
  {
    label: 'Infrastructure',
    items: [
      { key: 'infra', label: 'Hosts', icon: Server, route: '/infra' },
      { key: 'infrastructure', label: 'Ops', icon: Activity, route: '/uptime' },
    ],
  },
  {
    label: 'Explore',
    items: [
      { key: 'logs', label: 'Logs', icon: ScrollText, route: '/logs' },
      { key: 'traces', label: 'Traces', icon: Waypoints, route: '/traces' },
      { key: 'metrics', label: 'Metrics', icon: BarChart3, route: '/metrics' },
    ],
  },
  {
    label: 'Manage',
    items: [
      { key: 'data', label: 'Data', icon: Database, route: '/data' },
      { key: 'alerts', label: 'Alerts', icon: Bell, route: '/alerts' },
    ],
  },
]

function onSelect(item) {
  emit('select', item.key)
}
</script>

<template>
  <nav
    class="flex h-full w-[74px] shrink-0 flex-col items-center gap-1 border-r border-border bg-muted/40 py-3"
  >
    <div class="relative">
      <PhotonMark :size="30" />
      <span
        class="absolute -bottom-0.5 -right-0.5 size-2 rounded-full bg-brand ring-2 ring-background"
        aria-hidden="true"
      />
    </div>

    <div class="mt-6 flex w-full flex-col items-center gap-1">
      <template v-for="group in NAV_GROUPS" :key="group.label ?? 'root'">
        <div
          v-if="group.label"
          class="mb-1 mt-3 w-full px-1 text-center text-[9px] font-semibold uppercase tracking-wider text-muted-foreground/50"
        >
          {{ group.label }}
        </div>

        <button
          v-for="item in group.items"
          :key="item.key"
          type="button"
          :data-nav="item.key"
          :class="
            cn(
              'relative flex w-[58px] flex-col items-center gap-1 rounded-md py-2 transition-colors',
              item.key === active
                ? 'bg-brand-soft text-brand'
                : 'text-muted-foreground hover:bg-muted hover:text-foreground',
            )
          "
          @click="onSelect(item)"
        >
          <span
            v-if="item.key === active"
            class="absolute -left-2 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r-full bg-brand"
          />
          <component :is="item.icon" class="size-[18px]" />
          <span class="text-[10px] font-medium leading-none">{{ item.label }}</span>
        </button>
      </template>
    </div>

    <div class="mt-auto flex flex-col items-center gap-3 pb-1">
      <Tooltip>
        <TooltipTrigger as-child>
          <span
            class="flex size-8 cursor-default items-center justify-center"
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

      <button
        type="button"
        class="flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
        aria-label="Settings"
        @click="settingsOpen = true"
      >
        <Settings class="size-[18px]" />
      </button>

      <ThemeToggle />

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

    <SettingsDialog v-if="settingsOpen" v-model:open="settingsOpen" />
  </nav>
</template>
