<script setup lang="ts">
// The dry run's checks against the target host: arch, Docker version, memory, disk, each port,
// the agent's capability and reachability. A `fail` blocks the deploy; a `warn` is stated and
// the operator decides; `skip` says what could not be known yet, rather than pretending.
import { Check, X, TriangleAlert, Minus } from 'lucide-vue-next'
import { StatusPill } from '@/components/ui/status-pill'
import { PREFLIGHT_META } from '@/lib/deploy/templates'
import type { PreflightCheck } from '@/lib/core/api'

defineProps<{ checks: PreflightCheck[] }>()
const ICON = { pass: Check, fail: X, warn: TriangleAlert, skip: Minus }
</script>

<template>
  <ul class="flex flex-col divide-y divide-border rounded-xl border border-border bg-card shadow-1" data-testid="preflight">
    <li v-for="c in checks" :key="c.key" class="flex items-start gap-3 px-3 py-2 text-sm" :data-check="c.key" :data-status="c.status">
      <component :is="ICON[c.status]" class="mt-0.5 size-4 shrink-0" :class="c.status === 'pass' ? 'text-success' : c.status === 'fail' ? 'text-sev-error' : c.status === 'warn' ? 'text-sev-warn' : 'text-muted-foreground/60'" />
      <span class="min-w-0 flex-1">
        <span class="text-foreground">{{ c.label }}</span>
        <span class="block text-xs text-muted-foreground">{{ c.detail }}</span>
      </span>
      <StatusPill :tone="PREFLIGHT_META[c.status].tone" class="shrink-0">{{ PREFLIGHT_META[c.status].label }}</StatusPill>
    </li>
  </ul>
</template>
