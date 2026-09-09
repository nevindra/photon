<script setup lang="ts">
// The stack's compose services as observed on the host, as ONE table: service, image, its
// container(s) linking into the host's Containers drawer, state, health — and, on the row it
// belongs to, the drift that concerns it, so "lenz is restarting" and "lenz.state drifted" are
// read together. The observability links live on the row too: a compose service that shows up
// as an APM service gets its APM link, every row its logs. "Compose service" is the word here —
// these are not APM Services, though the two often share a name.
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { Activity, ExternalLink, ScrollText } from 'lucide-vue-next'
import { StatusDot } from '@/components/ui/status-dot'
import { StatusPill } from '@/components/ui/status-pill'
import { EmptyState } from '@/components/ui/empty-state'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { containerTone, STATE_META, HEALTH_META, shortImage } from '@/lib/deploy/containers'
import { driftLabel } from '@/lib/deploy/stacks'
import { correlate } from '@/lib/core/useCorrelate'
import { cn } from '@/lib/core/utils'
import type { DriftItem, Stack } from '@/lib/core/api'

const props = defineProps<{ stack: Stack }>()

const driftBy = computed(() => {
  const m = new Map<string, DriftItem[]>()
  for (const d of props.stack.drift) if (d.service) m.set(d.service, [...(m.get(d.service) ?? []), d])
  return m
})
// A service's logs: by its APM service name when it reports one (the OTLP logs carry it);
// otherwise the stack's logs narrowed by the container name as a body match.
const logsFor = (svc: { name: string; containers: { name: string }[] }) =>
  props.stack.apm_services.includes(svc.name)
    ? correlate({ path: '/logs', query: { q: `service.name:${svc.name}` } })
    : correlate({ path: '/logs', query: { q: `host.name:${props.stack.host} compose.project:${props.stack.name}${svc.containers[0] ? ` ${svc.containers[0].name}` : ''}` } })
const stackLogs = computed(() => correlate({ path: '/logs', query: { q: `host.name:${props.stack.host} compose.project:${props.stack.name}` } }))
const apmFor = (name: string) => (props.stack.apm_services.includes(name) ? correlate({ path: `/services/${encodeURIComponent(name)}` }) : null)
// APM services this stack produced that share no name with a compose service still deserve a
// way in — they go in the header, not lost.
const otherApm = computed(() => props.stack.apm_services.filter((s) => !props.stack.services.some((svc) => svc.name === s)))
</script>

<template>
  <section class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="stack-services">
    <header class="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2.5">
      <h3 class="text-sm font-medium text-foreground">Compose services</h3>
      <span class="text-xs text-muted-foreground">{{ stack.services.length }} · what the host runs for this stack<template v-if="stack.apm_services.length"> · {{ stack.apm_services.length }} reporting to APM</template><template v-else> · none reporting to APM yet</template></span>
      <div class="ml-auto flex flex-wrap items-center gap-1.5">
        <RouterLink v-for="s in otherApm" :key="s" :to="correlate({ path: `/services/${encodeURIComponent(s)}` })" class="inline-flex items-center gap-1 rounded-full border border-border px-2 py-0.5 text-xs text-foreground hover:border-brand hover:text-brand" :data-apm="s"><Activity class="size-3" /> {{ s }}</RouterLink>
        <RouterLink :to="stackLogs" class="inline-flex items-center gap-1 rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground hover:text-foreground" data-testid="stack-logs"><ScrollText class="size-3" /> This stack's logs <ExternalLink class="size-3 opacity-60" /></RouterLink>
      </div>
    </header>
    <Table v-if="stack.services.length" container-class="overflow-x-auto" class="text-sm">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead class="w-6" />
          <TableHead>Service</TableHead>
          <TableHead>Image</TableHead>
          <TableHead>Container</TableHead>
          <TableHead>State</TableHead>
          <TableHead>Health</TableHead>
          <TableHead>Drift</TableHead>
          <TableHead class="w-36 text-right">Observe</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <template v-for="svc in stack.services" :key="svc.name">
          <TableRow
            v-for="(c, i) in (svc.containers.length ? svc.containers : [null])"
            :key="c?.id ?? `${svc.name}-none`"
            :data-service="svc.name"
            :data-drifted="driftBy.has(svc.name) ? 'true' : undefined"
            :class="cn('border-border/60', driftBy.has(svc.name) && 'bg-sev-warn-soft/40 shadow-[inset_3px_0_0_hsl(var(--sev-warn))]')"
          >
            <TableCell class="py-2.5 pr-0"><StatusDot :tone="c ? containerTone(c as any) : 'neutral'" size="lg" /></TableCell>
            <TableCell class="py-2.5 font-medium text-foreground">{{ i === 0 ? svc.name : '' }}</TableCell>
            <TableCell class="max-w-[14rem] truncate py-2.5 font-mono text-xs text-muted-foreground" :title="svc.image">{{ i === 0 ? shortImage(svc.image) : '' }}</TableCell>
            <TableCell class="py-2.5 font-mono text-xs">
              <RouterLink v-if="c" :to="`/infra/${encodeURIComponent(stack.host)}/containers?c=${encodeURIComponent(c.name)}`" class="text-foreground hover:underline">{{ c.name }}</RouterLink>
              <span v-else class="font-sans text-muted-foreground">no container — {{ stack.status === 'running' ? 'missing on the host' : 'not applied yet' }}</span>
            </TableCell>
            <TableCell class="py-2.5"><StatusPill v-if="c" :tone="STATE_META[c.state].tone">{{ STATE_META[c.state].label }}</StatusPill><span v-else class="text-xs text-muted-foreground/60">—</span></TableCell>
            <TableCell class="py-2.5"><StatusPill v-if="c && HEALTH_META[c.health]" :tone="HEALTH_META[c.health]!.tone">{{ HEALTH_META[c.health]!.label }}</StatusPill><span v-else class="text-xs text-muted-foreground">{{ c ? 'no check' : '—' }}</span></TableCell>
            <TableCell class="py-2.5 text-xs">
              <div v-if="i === 0 && driftBy.get(svc.name)?.length" class="flex flex-col gap-0.5" data-testid="service-drift">
                <span v-for="(d, j) in driftBy.get(svc.name)" :key="j" class="whitespace-nowrap"><span class="text-sev-warn">{{ driftLabel(d) }}</span><span v-if="d.desired != null" class="text-muted-foreground"> · desired <span class="font-mono text-foreground">{{ d.desired }}</span></span></span>
              </div>
              <span v-else-if="i === 0" class="text-muted-foreground/60">—</span>
            </TableCell>
            <TableCell class="py-2 text-right">
              <div v-if="i === 0" class="inline-flex items-center gap-1">
                <RouterLink v-if="apmFor(svc.name)" :to="apmFor(svc.name)!" class="inline-flex h-6 items-center gap-1 rounded-md border border-border px-1.5 text-[11px] text-foreground hover:border-brand hover:text-brand" :data-apm="svc.name" title="This service on the Backend world"><Activity class="size-3" /> APM</RouterLink>
                <RouterLink :to="logsFor(svc)" class="inline-flex h-6 items-center gap-1 rounded-md border border-border px-1.5 text-[11px] text-muted-foreground hover:text-foreground" :data-logs="svc.name" title="This service's logs"><ScrollText class="size-3" /> Logs</RouterLink>
              </div>
            </TableCell>
          </TableRow>
        </template>
      </TableBody>
    </Table>
    <EmptyState v-else title="No services observed" :description="stack.status === 'gone' ? 'The stack was destroyed.' : 'The host has not reported this stack yet.'" class="py-6" />
  </section>
</template>
