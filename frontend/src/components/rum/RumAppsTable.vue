<script setup lang="ts">
// The ranked apps table for the RUM executive summary — one row per app, unhealthiest first (see
// `rankApps`). Each row shows a health status dot, traffic, the three core vitals coloured by their
// API rating, and error/affected-session counts, and drills into that app's vitals hero on click.
import { formatVital, type AppRow, type Rating, type VitalCell } from '@/lib/rum/rumSummary'
import { formatNumber } from '@/lib/core/format'
import { cn } from '@/lib/core/utils'

defineProps<{ rows: AppRow[] }>()
const emit = defineEmits<{ open: [app: string] }>()

const DOT: Record<Rating, string> = { good: 'bg-success', needs: 'bg-sev-warn', poor: 'bg-sev-error' }
const TEXT: Record<Rating, string> = { good: 'text-success', needs: 'text-sev-warn', poor: 'text-sev-error' }

const cellClass = (cell: VitalCell | null) =>
  cell && cell.rating ? TEXT[cell.rating] : 'text-muted-foreground'
</script>

<template>
  <div class="overflow-x-auto">
    <table class="w-full border-collapse text-sm">
      <thead>
        <tr class="border-b border-border text-[10px] uppercase tracking-wide text-muted-foreground">
          <th class="w-3 py-2"></th>
          <th class="py-2 text-left font-semibold">App</th>
          <th class="py-2 text-right font-semibold">Pageviews</th>
          <th class="py-2 text-right font-semibold">LCP</th>
          <th class="py-2 text-right font-semibold">INP</th>
          <th class="py-2 text-right font-semibold">CLS</th>
          <th class="py-2 text-right font-semibold">Errors</th>
          <th class="py-2 text-right font-semibold">Sessions</th>
          <th class="w-5 py-2"></th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="row in rows"
          :key="row.app"
          data-testid="rum-app-row"
          :data-app="row.app"
          role="button"
          tabindex="0"
          class="cursor-pointer border-b border-border/50 transition-colors last:border-0 hover:bg-muted/50 focus-visible:bg-muted/50 focus-visible:outline-none"
          @click="emit('open', row.app)"
          @keydown.enter="emit('open', row.app)"
        >
          <td class="py-2.5">
            <span
              class="inline-block h-2 w-2 rounded-full"
              :class="row.status ? DOT[row.status] : 'bg-muted-foreground'"
            />
          </td>
          <td class="py-2.5 font-medium text-foreground">{{ row.app }}</td>
          <td class="py-2.5 text-right font-mono tabular-nums text-muted-foreground">{{ formatNumber(row.pageviews) }}</td>
          <td :class="cn('py-2.5 text-right font-mono tabular-nums', cellClass(row.lcp))">
            {{ formatVital('web_vitals.lcp', row.lcp?.p75) }}
          </td>
          <td :class="cn('py-2.5 text-right font-mono tabular-nums', cellClass(row.inp))">
            {{ formatVital('web_vitals.inp', row.inp?.p75) }}
          </td>
          <td :class="cn('py-2.5 text-right font-mono tabular-nums', cellClass(row.cls))">
            {{ formatVital('web_vitals.cls', row.cls?.p75) }}
          </td>
          <td :class="cn('py-2.5 text-right font-mono tabular-nums', row.errors ? 'text-sev-error' : 'text-muted-foreground')">
            {{ formatNumber(row.errors) }}
          </td>
          <td class="py-2.5 text-right font-mono tabular-nums text-muted-foreground">{{ formatNumber(row.sessions) }}</td>
          <td class="py-2.5 text-right text-muted-foreground">›</td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
