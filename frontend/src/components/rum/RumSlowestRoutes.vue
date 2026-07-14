<script setup lang="ts">
// "Slowest routes" list for the RUM executive summary — routes across all apps ranked by LCP p75
// (see `slowestRoutes`). Each row's bar is scaled to the slowest route in view and coloured by the
// route's LCP rating; clicking drills into that route's page detail.
import { computed } from 'vue'
import { formatVital, type RouteRow, type Rating } from '@/lib/rum/rumSummary'
import { cn } from '@/lib/core/utils'

const props = defineProps<{ routes: RouteRow[] }>()
const emit = defineEmits<{ open: [payload: { app: string; route: string }] }>()

const TEXT: Record<Rating, string> = { good: 'text-success', needs: 'text-sev-warn', poor: 'text-sev-error' }
const BAR: Record<Rating, string> = { good: 'bg-success', needs: 'bg-sev-warn', poor: 'bg-sev-error' }

const maxLcp = computed(() => Math.max(1, ...props.routes.map((r) => r.lcp_p75 ?? 0)))
const widthOf = (r: RouteRow) => Math.max(6, ((r.lcp_p75 ?? 0) / maxLcp.value) * 100) + '%'
</script>

<template>
  <ul class="flex flex-col">
    <li
      v-for="r in routes"
      :key="r.app + ':' + r.route"
      data-testid="rum-route"
      role="button"
      tabindex="0"
      class="flex cursor-pointer items-center gap-3 border-b border-border/50 py-2 transition-colors last:border-0 hover:bg-muted/40 focus-visible:bg-muted/40 focus-visible:outline-none"
      @click="emit('open', { app: r.app, route: r.route })"
      @keydown.enter="emit('open', { app: r.app, route: r.route })"
    >
      <span class="min-w-0 flex-1 truncate font-mono text-xs text-foreground">{{ r.route }}</span>
      <span class="hidden shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground sm:inline">
        {{ r.app }}
      </span>
      <span class="h-1.5 w-16 shrink-0 overflow-hidden rounded-full bg-muted">
        <span class="block h-full rounded-full" :class="r.rating ? BAR[r.rating] : 'bg-muted-foreground'" :style="{ width: widthOf(r) }" />
      </span>
      <span :class="cn('w-12 shrink-0 text-right font-mono text-xs tabular-nums', r.rating ? TEXT[r.rating] : 'text-muted-foreground')">
        {{ formatVital('web_vitals.lcp', r.lcp_p75) }}
      </span>
    </li>
  </ul>
</template>
