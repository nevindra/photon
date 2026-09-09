<script setup lang="ts">
// The daemon's recent events (create / start / die / OOM / health), newest first. The same
// events land in the logs signal as OTLP, so an OOM-kill reaches the Attention Board without
// anyone opening this page; the feed is for the operator who IS on this page.
//
// A health check execs every ten seconds and Docker reports each exec as three events, so on
// a quiet host the raw feed is one command repeating. `foldEvents` turns one actor's exec
// chatter into a single row — the command, how many, how often — that opens to the raw events
// on click, so the rows left are the ones where something changed.
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ExternalLink, ChevronRight, Repeat } from 'lucide-vue-next'
import { StatusPill } from '@/components/ui/status-pill'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { eventTone, eventLabel, eventDetail, shortImage, foldEvents, everyLabel } from '@/lib/deploy/containers'
import { ageMs } from '@/lib/deploy/audit'
import { correlate } from '@/lib/core/useCorrelate'
import { cn } from '@/lib/core/utils'
import type { DockerEvent } from '@/lib/core/api'

const props = defineProps<{ host: string; events: DockerEvent[]; limit?: number }>()
const emit = defineEmits<{ open: [actor: string] }>()
const router = useRouter()
const shown = computed(() => foldEvents(props.events).slice(0, props.limit ?? 12))
const opened = ref<Set<string>>(new Set())
function toggle(actor: string): void {
  const next = new Set(opened.value)
  if (next.has(actor)) next.delete(actor)
  else next.add(actor)
  opened.value = next
}

function openInLogs(): void {
  router.push(correlate({ path: '/logs', query: { q: `host.name:${props.host} docker.event:*` } }))
}
</script>

<template>
  <section class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="docker-events">
    <header class="flex items-center gap-2 border-b border-border px-4 py-2.5">
      <h3 class="text-sm font-medium text-foreground">Docker events</h3>
      <span class="text-xs text-muted-foreground">from the daemon, also shipped as logs</span>
      <Button variant="ghost" size="sm" class="ml-auto h-7 text-xs" @click="openInLogs"><ExternalLink class="size-3.5" /> Open in Logs</Button>
    </header>
    <ul v-if="shown.length" class="divide-y divide-border/60">
      <template v-for="item in shown" :key="item.kind === 'fold' ? `fold:${item.actor}` : item.event.id">
        <!-- A fold: one row for a repeating exec, opening to the raw events beneath it. -->
        <li v-if="item.kind === 'fold'" class="bg-muted/40" data-testid="docker-event-fold" :data-actor="item.actor" :data-count="item.count">
          <button type="button" class="flex w-full items-center gap-3 px-4 py-1.5 text-left text-xs hover:bg-muted/70 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring" :aria-expanded="opened.has(item.actor)" @click="toggle(item.actor)">
            <span class="w-16 shrink-0 tabular-nums text-muted-foreground" :title="new Date(item.ts_ms).toISOString()">{{ ageMs(item.ts_ms) }}</span>
            <ChevronRight :class="cn('size-3.5 shrink-0 text-muted-foreground transition-transform', opened.has(item.actor) && 'rotate-90')" />
            <Repeat class="size-3.5 shrink-0 text-muted-foreground" />
            <span class="shrink-0 text-foreground">Health check</span>
            <span class="shrink-0 font-mono text-foreground">{{ item.actor }}</span>
            <span v-if="item.command" class="min-w-0 truncate font-mono text-muted-foreground" :title="item.command">{{ item.command }}</span>
            <span class="ml-auto shrink-0 whitespace-nowrap text-muted-foreground"><template v-if="item.every_ms">{{ everyLabel(item.every_ms) }} · </template>{{ item.count }} events folded</span>
          </button>
          <ul v-if="opened.has(item.actor)" class="divide-y divide-border/60 border-t border-border/60 bg-card">
            <li v-for="e in item.events" :key="e.id" class="flex items-center gap-3 py-1.5 pl-12 pr-4 text-xs" data-testid="docker-event" :data-action="e.action">
              <span class="w-16 shrink-0 tabular-nums text-muted-foreground">{{ ageMs(e.ts_ms) }}</span>
              <StatusPill :tone="eventTone(e.action, e.attributes)" class="w-32 shrink-0 justify-center whitespace-nowrap">{{ eventLabel(e.action, e.attributes) }}</StatusPill>
              <span v-if="eventDetail(e.action)" class="min-w-0 truncate font-mono text-muted-foreground" :title="eventDetail(e.action) ?? undefined">{{ eventDetail(e.action) }}</span>
            </li>
          </ul>
        </li>
        <li v-else class="flex items-center gap-3 px-4 py-1.5 text-xs" data-testid="docker-event" :data-action="item.event.action">
          <span class="w-16 shrink-0 tabular-nums text-muted-foreground" :title="new Date(item.event.ts_ms).toISOString()">{{ ageMs(item.event.ts_ms) }}</span>
          <!-- The pill is one line, always: the verb only, at a fixed width so the actor column
               lines up down the feed. An exec's command line goes in its own truncating span with
               the full text on hover — it is the one field that can run to hundreds of characters. -->
          <StatusPill :tone="eventTone(item.event.action, item.event.attributes)" class="w-32 shrink-0 justify-center whitespace-nowrap">{{ eventLabel(item.event.action, item.event.attributes) }}</StatusPill>
          <button type="button" class="max-w-56 shrink-0 truncate font-mono text-foreground hover:underline" @click="emit('open', item.event.actor)">{{ item.event.actor }}</button>
          <span v-if="eventDetail(item.event.action)" class="min-w-0 truncate font-mono text-muted-foreground" :title="eventDetail(item.event.action) ?? undefined" data-testid="docker-event-detail">{{ eventDetail(item.event.action) }}</span>
          <span v-if="item.event.attributes.image" class="min-w-0 truncate font-mono text-muted-foreground/70">{{ shortImage(item.event.attributes.image) }}</span>
          <span class="ml-auto shrink-0 text-muted-foreground/60">{{ item.event.type }}</span>
        </li>
      </template>
    </ul>
    <EmptyState v-else title="No recent events" class="h-auto py-8" />
  </section>
</template>
