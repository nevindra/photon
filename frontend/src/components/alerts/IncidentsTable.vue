<script setup lang="ts">
// AlertsView's "incidents" tab body (Task 15). Reference layout: the approved "Variant 1" Incidents
// mockup (.superpowers/brainstorm/…/alerts-final.html) — one card, a "Triggered now" group (red pill
// + a subtle left accent bar) above a "Resolved · 24h" group, no full-width banner, no Silence action
// (v1 non-goal). Mirrors AlertRuleRow.vue's just-landed conventions (Task 13) for a consistent look
// across the two tabs: `signalColor`/`StatusPill` for chips/pills, and the same svc:/app:/group_by
// "condition scope" fallback for a rule's aggregate series.
//
// Incidents only carry `peak_value` + a human `summary` string, not a live threshold, so value/
// threshold is reconstructed by joining each incident's `rule_id` against `useRules()` — a rule
// that's since been deleted just falls back to showing the peak value alone. The incidents API has
// no time-window filter (`AlertIncidentsFilter` is status/rule_id/limit only), so "24h" is enforced
// client-side over the resolved list to keep that group header honest.
import { computed } from 'vue'
import { Table, TableBody, TableRow, TableCell } from '@/components/ui/table'
import { StatusPill } from '@/components/ui/status-pill'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { signalColor } from '@/lib/core/signalMeta'
import { useIncidents, useRules } from '@/lib/alertsQueries'
import type { AlertIncident, AlertRule } from '@/lib/core/api'

const RESOLVED_WINDOW_MS = 24 * 60 * 60 * 1000

const triggeredQuery = useIncidents({ status: 'triggered' })
const resolvedQuery = useIncidents({ status: 'resolved', limit: 50 })
const rulesQuery = useRules()

const rulesById = computed<Record<string, AlertRule>>(() =>
  Object.fromEntries((rulesQuery.data.value ?? []).map((r) => [r.id, r])),
)

// Longest-running first — the most urgent triggered incident surfaces at the top.
const triggered = computed<AlertIncident[]>(() =>
  [...(triggeredQuery.data.value ?? [])].sort((a, b) => a.started_at - b.started_at),
)
// Most-recently-resolved first, scoped to the last 24h (the API itself has no time filter).
const resolved = computed<AlertIncident[]>(() => {
  const cutoff = Date.now() - RESOLVED_WINDOW_MS
  return (resolvedQuery.data.value ?? [])
    .filter((i) => i.ended_at != null && i.ended_at >= cutoff)
    .sort((a, b) => (b.ended_at ?? 0) - (a.ended_at ?? 0))
})

const isLoading = computed(() => triggeredQuery.isLoading.value || resolvedQuery.isLoading.value)
const isError = computed(() => triggeredQuery.isError.value || resolvedQuery.isError.value)

function ruleFor(incident: AlertIncident): AlertRule | undefined {
  return rulesById.value[incident.rule_id]
}
function ruleName(incident: AlertIncident): string {
  return ruleFor(incident)?.name ?? incident.rule_id
}
function ruleSignal(incident: AlertIncident): string | null {
  return ruleFor(incident)?.signal ?? null
}
// Incident's own `peak_value` vs the rule's condition threshold. `threshold` is present on every
// arm of the `AlertCondition` union, so no per-signal narrowing is needed here.
function threshold(incident: AlertIncident): number | null {
  return ruleFor(incident)?.condition.threshold ?? null
}

// `series_key` is a canonical "k=v,k=v" string ("" for an aggregate series) — shorten dotted keys
// (`host.name` -> `host`) into a compact label. Falls back to the rule's own scope hint (mirrors
// AlertRuleRow's `conditionScope`) when the incident is on the rule's single aggregate series.
function seriesLabel(incident: AlertIncident): string {
  if (incident.series_key) {
    return incident.series_key
      .split(',')
      .map((pair) => {
        const [k, v] = pair.split('=')
        return `${k?.split('.').pop() ?? k}: ${v ?? ''}`
      })
      .join(', ')
  }
  const c = ruleFor(incident)?.condition
  if (!c) return '—'
  if (c.signal === 'traces') return `svc: ${c.service}`
  if (c.signal === 'rum') return `app: ${c.app_id}`
  if (c.signal === 'metrics') return c.metric_name
  return 'all logs'
}

function fmtVal(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—'
  return Number.isInteger(n) ? String(n) : String(Math.round(n * 100) / 100)
}

// Compact "time since" label for a ms epoch timestamp, e.g. "12m ago" / "3h ago" / "2d ago".
function agoLabel(ms: number): string {
  const secs = Math.max(0, Math.round((Date.now() - ms) / 1000))
  if (secs < 60) return 'just now'
  const mins = Math.round(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.round(mins / 60)
  if (hours < 24) return `${hours}h ago`
  return `${Math.round(hours / 24)}d ago`
}

// Compact elapsed-duration label for a millisecond span, e.g. "8m" / "1h 4m" / "2d".
function durationLabel(ms: number): string {
  const secs = Math.max(0, Math.round(ms / 1000))
  if (secs < 60) return `${secs}s`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return mins % 60 ? `${hours}h ${mins % 60}m` : `${hours}h`
  return `${Math.floor(hours / 24)}d`
}
</script>

<template>
  <div>
    <p v-if="isLoading" class="text-sm text-muted-foreground"><Spinner size="sm">Loading…</Spinner></p>
    <p v-else-if="isError" class="text-sm text-destructive">Failed to load incidents.</p>
    <EmptyState
      v-else-if="!triggered.length && !resolved.length"
      title="No incidents"
      description="Nothing has triggered recently."
    />
    <div v-else class="overflow-hidden rounded-lg border border-border bg-card shadow-1">
      <div
        class="flex items-center gap-1.5 border-b border-border bg-muted/40 px-3 py-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground"
        data-testid="incidents-group-triggered"
      >
        Triggered now <span>· {{ triggered.length }}</span>
      </div>
      <Table v-if="triggered.length">
        <TableBody>
          <TableRow
            v-for="incident in triggered"
            :key="incident.id"
            class="bg-sev-error-soft/40 shadow-[inset_2.5px_0_0_hsl(var(--sev-error))]"
          >
            <TableCell class="w-[100px]">
              <StatusPill tone="error">triggered</StatusPill>
            </TableCell>
            <TableCell>
              <span class="font-medium text-foreground">{{ ruleName(incident) }}</span>
              <span
                v-if="ruleSignal(incident)"
                class="ml-2 inline-flex items-center gap-1.5 rounded-md border border-border bg-muted px-2 py-0.5 align-middle text-xs font-medium capitalize"
              >
                <span class="size-1.5 shrink-0 rounded-sm" :style="{ background: signalColor(ruleSignal(incident)!) }" />
                {{ ruleSignal(incident) }}
              </span>
            </TableCell>
            <TableCell>
              <code class="whitespace-nowrap rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-xs text-muted-foreground">{{ seriesLabel(incident) }}</code>
            </TableCell>
            <TableCell class="whitespace-nowrap font-mono text-xs">
              <b class="text-sev-error">{{ fmtVal(incident.peak_value) }}</b>
              <span v-if="threshold(incident) != null" class="text-muted-foreground"> / {{ fmtVal(threshold(incident)) }}</span>
            </TableCell>
            <TableCell class="text-right whitespace-nowrap text-xs text-muted-foreground">{{ agoLabel(incident.started_at) }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
      <p v-else class="px-3 py-4 text-sm text-muted-foreground">No incidents currently triggered.</p>

      <div
        class="flex items-center gap-1.5 border-t border-b border-border bg-muted/40 px-3 py-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground"
        data-testid="incidents-group-resolved"
      >
        Resolved · 24h
      </div>
      <Table v-if="resolved.length">
        <TableBody>
          <TableRow v-for="incident in resolved" :key="incident.id">
            <TableCell class="w-[100px]">
              <StatusPill tone="success">resolved</StatusPill>
            </TableCell>
            <TableCell class="font-medium text-foreground">{{ ruleName(incident) }}</TableCell>
            <TableCell>
              <code class="whitespace-nowrap rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-xs text-muted-foreground">{{ seriesLabel(incident) }}</code>
            </TableCell>
            <TableCell class="whitespace-nowrap font-mono text-xs text-muted-foreground">peak {{ fmtVal(incident.peak_value) }}</TableCell>
            <TableCell class="whitespace-nowrap text-xs text-muted-foreground">{{ agoLabel(incident.started_at) }}</TableCell>
            <TableCell class="text-right whitespace-nowrap text-xs text-muted-foreground">
              {{ incident.ended_at != null ? durationLabel(incident.ended_at - incident.started_at) : '—' }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
      <p v-else class="px-3 py-4 text-sm text-muted-foreground">No incidents resolved in the last 24h.</p>
    </div>
  </div>
</template>
