<script setup lang="ts">
// `docker logs`, tail and follow, without SSH. The tail loads once; Follow (on by default for a
// running container) polls for new lines each second and pins the view to the bottom until the
// reader scrolls up, at which point new lines arrive quietly and a "↓ N new" chip offers the way
// back. stderr is tinted; a filter narrows the visible lines client-side.
import { computed, nextTick, ref, watch } from 'vue'
import { ArrowDown, Copy, Search, ExternalLink } from 'lucide-vue-next'
import { RouterLink, useRouter } from 'vue-router'
import { correlate } from '@/lib/core/useCorrelate'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { useContainerLogs } from '@/lib/deploy/deployQueries'
import { useCopy } from '@/lib/core/useCopy'
import { cn } from '@/lib/core/utils'

const props = defineProps<{ host: string; containerId: string; containerName: string; running: boolean }>()

const follow = ref(props.running)
const wrap = ref(false)
const filter = ref('')
const { lines, truncated, loading } = useContainerLogs(() => props.host, () => props.containerId, { tail: 200, follow })
const { copy } = useCopy()
const router = useRouter()

// Trace ids in a log line (`trace=…`, `trace_id=…`, `traceparent=…`) link into the waterfall —
// the one thing a plain `docker logs` can never do.
const TRACE_RE = /\b(trace(?:_id|parent)?=)([0-9a-f]{12,32})\b/gi
interface Piece {
  text: string
  trace?: string
}
function pieces(text: string): Piece[] {
  const out: Piece[] = []
  let last = 0
  for (const m of text.matchAll(TRACE_RE)) {
    const i = m.index ?? 0
    if (i > last) out.push({ text: text.slice(last, i) })
    out.push({ text: m[1] })
    out.push({ text: m[2], trace: m[2] })
    last = i + m[0].length
  }
  if (last < text.length) out.push({ text: text.slice(last) })
  return out
}
function openInLogs(): void {
  router.push(correlate({ path: '/logs', query: { q: `container.name:${props.containerName}` } }))
}

const visible = computed(() => {
  const q = filter.value.trim().toLowerCase()
  return q ? lines.value.filter((l) => l.text.toLowerCase().includes(q)) : lines.value
})

const scroller = ref<HTMLElement | null>(null)
const pinned = ref(true)
const unseen = ref(0)
function onScroll(): void {
  const el = scroller.value
  if (!el) return
  pinned.value = el.scrollHeight - el.scrollTop - el.clientHeight < 24
  if (pinned.value) unseen.value = 0
}
function toBottom(): void {
  const el = scroller.value
  if (el) el.scrollTop = el.scrollHeight
  pinned.value = true
  unseen.value = 0
}
watch(
  () => visible.value.length,
  async (n, prev) => {
    await nextTick()
    if (pinned.value) toBottom()
    else if (n > prev) unseen.value += n - prev
  },
)

function stamp(ms: number): string {
  const d = new Date(ms)
  return d.toLocaleTimeString(undefined, { hour12: false }) + '.' + String(d.getMilliseconds()).padStart(3, '0')
}
function copyAll(): void {
  copy(visible.value.map((l) => `${new Date(l.ts_ms).toISOString()} ${l.text}`).join('\n'), `${visible.value.length} log lines`)
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col" data-testid="container-logs">
    <div class="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2">
      <div class="relative min-w-[10rem] flex-1">
        <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input v-model="filter" placeholder="Filter lines…" class="h-7 pl-8 text-xs" data-testid="log-filter" />
      </div>
      <label class="flex items-center gap-1.5 text-xs text-foreground" for="log-follow">
        <Switch id="log-follow" :model-value="follow" data-testid="log-follow" class="scale-90" @update:model-value="follow = $event" />
        Follow
      </label>
      <Button variant="outline" size="sm" class="h-7 px-2 text-xs" :class="wrap && 'bg-muted'" data-testid="log-wrap" @click="wrap = !wrap">Wrap</Button>
      <Button variant="outline" size="sm" class="h-7 px-2 text-xs" data-testid="log-copy" @click="copyAll"><Copy class="size-3.5" /> Copy</Button>
      <Button variant="outline" size="sm" class="h-7 px-2 text-xs" data-testid="log-open-logs" title="The same lines in the Logs explorer, with the grammar and facets" @click="openInLogs"><ExternalLink class="size-3.5" /> Logs</Button>
    </div>

    <div v-if="loading && !lines.length" class="flex flex-1 items-center justify-center py-10"><Spinner size="md">Fetching the tail…</Spinner></div>
    <div v-else class="relative min-h-0 flex-1">
      <div ref="scroller" class="h-full overflow-auto bg-surface-1 px-3 py-2 font-mono text-[11.5px] leading-5" data-testid="log-scroller" @scroll="onScroll">
        <p v-if="truncated" class="mb-1 text-[10px] uppercase tracking-wider text-muted-foreground/60">showing the last 200 lines</p>
        <div v-for="(l, i) in visible" :key="l.ts_ms + ':' + i" :class="cn('flex gap-3', l.stream === 'stderr' ? 'text-sev-error' : 'text-foreground')" :data-stream="l.stream">
          <span class="shrink-0 select-none tabular-nums text-muted-foreground/60">{{ stamp(l.ts_ms) }}</span>
          <span :class="wrap ? 'whitespace-pre-wrap break-all' : 'whitespace-pre'"><template v-for="(p, j) in pieces(l.text)" :key="j"><RouterLink v-if="p.trace" :to="correlate({ path: `/traces/${p.trace}` })" class="text-brand underline decoration-brand/40 hover:decoration-brand" data-testid="log-trace-link">{{ p.text }}</RouterLink><template v-else>{{ p.text }}</template></template></span>
        </div>
        <p v-if="!visible.length" class="py-8 text-center text-xs text-muted-foreground">{{ lines.length ? 'No lines match the filter.' : running ? 'No output yet.' : 'This container wrote nothing before it stopped.' }}</p>
      </div>
      <button v-if="!pinned && unseen" type="button" class="absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full bg-primary px-3 py-1 text-xs font-medium text-primary-foreground shadow-2" data-testid="log-unseen" @click="toBottom">
        <ArrowDown class="mr-1 inline size-3" />{{ unseen }} new
      </button>
    </div>
    <div class="flex items-center gap-2 border-t border-border px-4 py-1.5 text-[11px] text-muted-foreground">
      <span class="tabular-nums">{{ visible.length }} lines</span>
      <span v-if="follow && running" class="inline-flex items-center gap-1 text-success"><span class="size-1.5 animate-pulse rounded-full bg-success" /> live</span>
      <span v-else-if="!running">container is not running — this is its final output</span>
      <span class="ml-auto">the agent streams these on its own outbound sync; nothing dials into the host</span>
    </div>
  </div>
</template>
