<script setup lang="ts">
// Triggered incidents that resolve to no Subject: a rule genuinely about the fleet (metrics with
// no `group_by`), or one whose place is not recoverable from the rule. They get their own banner
// above everything else and are never hidden — a page that goes quiet about something that fired
// is worse than no page.
import { TriangleAlert, ArrowRight } from 'lucide-vue-next'
import type { AlertIncident, AlertRule } from '@/lib/core/api'

defineProps<{ items: { incident: AlertIncident; rule?: AlertRule }[] }>()
defineEmits<{ (e: 'open'): void }>()

function describe(item: { incident: AlertIncident; rule?: AlertRule }): string {
  return item.incident.summary || item.rule?.name || `Rule ${item.incident.rule_id}`
}
</script>

<template>
  <button
    type="button"
    class="flex w-full items-center gap-3 rounded-xl border border-sev-error/30 bg-sev-error-soft px-4 py-3 text-left transition-colors hover:bg-sev-error-soft/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    data-testid="unattributed-incidents"
    @click="$emit('open')"
  >
    <TriangleAlert class="size-4 shrink-0 text-sev-error" />
    <span class="shrink-0 text-[13px] font-semibold text-sev-error">
      {{ items.length }} incident{{ items.length > 1 ? 's' : '' }} with no subject
    </span>
    <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
      {{ items.map(describe).join(' · ') }}
    </span>
    <span class="inline-flex shrink-0 items-center gap-1 text-xs font-medium text-sev-error">
      Alerts
      <ArrowRight class="size-3.5" />
    </span>
  </button>
</template>
