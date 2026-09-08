<script setup lang="ts">
// The stacks list grouped under its hosts — "what runs at each site" literally. Each host row
// carries the Site and the agent's state; each stack row answers the ops team's question in one
// line: status, name (with the failure or drift under it, where it is read), the recipe that
// produced it (and whether a newer version exists), the services BY NAME with their own state,
// the generation (with the gap when a newer one waits) and what happened last. Rows stay flat;
// click drills in. Columns are bounded so a wide screen does not spread them across the page.
import { useRouter, RouterLink } from 'vue-router'
import { ChevronRight, Server } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { StatusDot } from '@/components/ui/status-dot'
import { EmptyState } from '@/components/ui/empty-state'
import StackStatusPill from './StackStatusPill.vue'
import { generationLabel, hasPendingGeneration, sourceLabel, driftSummary, groupStacksByHost, lastChangeLabel } from '@/lib/deploy/stacks'
import { containerTone } from '@/lib/deploy/containers'
import { AGENT_STATUS_META } from '@/lib/deploy/agentStatus'
import { semverCompare } from '@/lib/deploy/templates'
import { cn } from '@/lib/core/utils'
import type { ManagedHost, Stack } from '@/lib/core/api'

const props = defineProps<{
  stacks: Stack[]
  loading?: boolean
  showSite?: boolean
  /** The managed hosts, for the agent's state on each host row. */
  agents?: ManagedHost[]
  /** template id → its latest version, so a stack behind it can say so. */
  latestVersions?: Record<string, string>
}>()
const router = useRouter()
function open(s: Stack): void {
  router.push(`/deploy/stacks/${encodeURIComponent(s.id)}`)
}
const agentFor = (host: string): ManagedHost | null => props.agents?.find((h) => h.host === host) ?? null
function newerVersion(s: Stack): string | null {
  if (s.source !== 'template' || !s.template_id || !s.template_version) return null
  const latest = props.latestVersions?.[s.template_id]
  return latest && semverCompare(latest, s.template_version) > 0 ? latest : null
}
const COLS = 'grid-cols-[7rem_minmax(0,1.4fr)_minmax(0,1.3fr)_minmax(0,1.6fr)_6.5rem_minmax(0,1.1fr)_1.5rem]'
</script>

<template>
  <div class="flex max-w-[1480px] flex-col gap-2.5" data-testid="stacks-table">
    <section v-for="g in groupStacksByHost(stacks)" :key="g.host" class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="host-group" :data-host="g.host">
      <header class="flex flex-wrap items-center gap-2.5 border-b border-border bg-muted/40 px-3.5 py-2 text-sm">
        <span class="inline-flex items-center gap-1.5 font-mono text-[12.5px] font-medium text-foreground"><Server class="size-3.5 text-muted-foreground" /> {{ g.host }}</span>
        <Badge v-if="g.site" variant="outline">{{ g.site }}</Badge>
        <span v-if="agentFor(g.host)" class="inline-flex items-center gap-1.5 text-xs text-muted-foreground" :title="AGENT_STATUS_META[agentFor(g.host)!.status].hint">
          <StatusDot :tone="AGENT_STATUS_META[agentFor(g.host)!.status].tone" size="sm" /> agent {{ AGENT_STATUS_META[agentFor(g.host)!.status].label.toLowerCase() }}
        </span>
        <span class="ml-auto text-xs text-muted-foreground">{{ g.stacks.length }} {{ g.stacks.length === 1 ? 'stack' : 'stacks' }}</span>
      </header>
      <div :class="cn('hidden h-7 items-center gap-3 border-b border-border px-3.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground sm:grid', COLS)">
        <span>Status</span><span>Stack</span><span>Source</span><span>Services</span><span>Generation</span><span>Last change</span><span />
      </div>
      <div
        v-for="(s, i) in g.stacks"
        :key="s.id"
        :class="cn('grid cursor-pointer items-center gap-2 px-3.5 py-2.5 text-sm transition-colors hover:bg-muted/30 sm:gap-3', COLS, i > 0 && 'border-t border-border/60')"
        data-testid="stack-row"
        :data-stack="s.id"
        :data-status="s.status"
        @click="open(s)"
      >
        <span><StackStatusPill :status="s.status" /></span>
        <span class="min-w-0">
          <RouterLink :to="`/deploy/stacks/${encodeURIComponent(s.id)}`" class="font-semibold text-foreground hover:underline" @click.stop>{{ s.name }}</RouterLink>
          <span v-if="s.status === 'failed' && s.last_error" class="line-clamp-1 font-mono text-[11px] text-sev-error" :title="s.last_error">{{ s.last_error }}</span>
          <span v-else-if="s.drift.length" class="line-clamp-1 text-[11px] text-sev-warn" :title="driftSummary(s.drift)">{{ driftSummary(s.drift) }}</span>
          <span v-else-if="hasPendingGeneration(s) && s.status !== 'applying'" class="line-clamp-1 text-[11px] text-muted-foreground">generation {{ s.desired_generation }} {{ s.status === 'pending' ? 'waits for the next sync' : 'rendered, not applied' }}</span>
          <span v-else-if="s.apm_services.length" class="line-clamp-1 text-[11px] text-muted-foreground">APM: <span class="font-mono">{{ s.apm_services.join(', ') }}</span></span>
        </span>
        <span class="flex min-w-0 flex-wrap items-center gap-1">
          <Badge variant="outline" class="font-normal text-muted-foreground">{{ s.source }}</Badge>
          <span v-if="s.source === 'template'" class="truncate font-mono text-xs text-foreground">{{ sourceLabel(s) }}</span>
          <Badge v-if="newerVersion(s)" variant="brand" class="font-mono" :title="`${newerVersion(s)} is the template's latest version`" data-testid="newer-version">{{ newerVersion(s) }} available</Badge>
        </span>
        <span class="flex min-w-0 flex-wrap items-center gap-1" :title="s.services.map((x) => `${x.name}: ${x.containers.map((c) => c.state).join(', ') || 'no container'}`).join('\n')">
          <span v-for="svc in s.services" :key="svc.name" class="inline-flex h-5 items-center gap-1.5 rounded-md bg-muted px-1.5 font-mono text-[10.5px] text-foreground" :data-service="svc.name">
            <StatusDot :tone="svc.containers.length ? containerTone(svc.containers[0] as any) : 'neutral'" size="sm" /> {{ svc.name }}
          </span>
          <span v-if="!s.services.length" class="text-xs text-muted-foreground/60">—</span>
        </span>
        <span class="font-mono text-xs tabular-nums" :class="hasPendingGeneration(s) ? 'text-brand' : 'text-muted-foreground'">{{ generationLabel(s) }}</span>
        <span class="truncate text-xs text-muted-foreground" :title="new Date(s.updated_ms).toLocaleString()">{{ lastChangeLabel(s) }}</span>
        <span class="text-right text-muted-foreground"><ChevronRight class="inline size-3.5" /></span>
      </div>
    </section>
    <EmptyState v-if="!loading && !stacks.length" title="No stacks" description="Deploy one from a template, write a compose file, or adopt a project already running on a host." class="h-auto rounded-xl border border-border bg-card py-12" />
  </div>
</template>
