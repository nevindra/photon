<script setup lang="ts">
// Everything `docker inspect` would tell an operator, minus the secrets: environment values that
// look secret are masked by the agent before they leave the host, because a value in a browser
// is a value in a screenshot.
//
// The tab is built for one habit: an operator lands here hunting ONE string — an env var, a
// mount path, a label — in a hundred rows. So a find box sits at the top, every section filters
// to the rows that contain the query with each hit marked, and a section with nothing left
// folds away. Rows are key ▸ value, keys in a fixed column so the eye can run down them, values
// wrapping rather than truncating (a truncated value is exactly the part you were looking for),
// and every value that is worth pasting somewhere gets a copy button on hover.
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { Search, X, Copy, Lock, ArrowRight } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { EmptyState } from '@/components/ui/empty-state'
import Highlight from './Highlight.vue'
import { formatPort, shortDigest, STATE_META } from '@/lib/deploy/containers'
import { stampMs } from '@/lib/deploy/audit'
import { normalizeQuery, rowMatches } from '@/lib/deploy/detailsSearch'
import { useCopy } from '@/lib/core/useCopy'
import type { Container } from '@/lib/core/api'

const props = defineProps<{ container: Container }>()
const { copy } = useCopy()

const MASK = '••••••••'

const raw = ref('')
const q = computed(() => normalizeQuery(raw.value))
const searching = computed(() => q.value.length > 0)

interface Row {
  key: string
  // The searchable, copyable text. Chips render `chips` instead but still search on `value`.
  value: string
  mono?: boolean
  // A quieter trailing note (the image digest, the exit code) — searchable, not copied.
  note?: string
  chips?: string[]
  copy?: boolean
  masked?: boolean
  link?: string
}

const c = computed(() => props.container)

const overview = computed<Row[]>(() => {
  const x = c.value
  const rows: Row[] = [
    { key: 'ID', value: x.id, mono: true, copy: true },
    { key: 'Image', value: x.image, mono: true, copy: true, note: x.image_id ? `@ ${shortDigest(x.image_id)}` : undefined },
    { key: 'Command', value: x.command, mono: true, copy: true },
    {
      key: 'State',
      value: `${STATE_META[x.state].label} — ${x.status_text}`,
      note: x.exit_code != null ? `exit code ${x.exit_code}` : undefined,
    },
    { key: 'Created', value: stampMs(x.created_ms) },
    { key: 'Started', value: x.started_ms ? stampMs(x.started_ms) : '—' },
  ]
  if (x.finished_ms) rows.push({ key: 'Finished', value: stampMs(x.finished_ms) })
  rows.push({ key: 'Restart policy', value: x.restart_policy, note: `restarted ${x.restart_count}×` })
  if (x.compose_project) rows.push({ key: 'Compose', value: `${x.compose_project} / ${x.compose_service}`, mono: true })
  rows.push({ key: 'Networks', value: x.networks.join(' '), chips: x.networks })
  return rows
})

const ports = computed<Row[]>(() =>
  c.value.ports.length ? [{ key: 'Published', value: c.value.ports.map(formatPort).join(' '), chips: c.value.ports.map(formatPort) }] : [],
)

// Env sorted by name: the daemon hands them back in whatever order the image declared them,
// which is no order to scan by.
const env = computed<Row[]>(() =>
  [...c.value.env_keys]
    .sort((a, b) => a.localeCompare(b))
    .map((k) => {
      const v = c.value.env[k] ?? ''
      const masked = v === MASK
      return { key: k, value: masked ? '' : v, mono: true, copy: !masked, masked }
    }),
)
const maskedCount = computed(() => env.value.filter((r) => r.masked).length)

const labels = computed<Row[]>(() =>
  Object.entries(c.value.labels)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([k, v]) => ({
      key: k,
      value: v,
      mono: true,
      copy: true,
      link: k === 'io.photon.stack' ? `/deploy/stacks/${encodeURIComponent(v)}` : undefined,
    })),
)

const mounts = computed(() => c.value.mounts.filter((m) => rowMatches([m.type, m.source, m.destination], q.value)))

function visible(rows: Row[]): Row[] {
  return rows.filter((r) => rowMatches([r.key, r.value, r.note], q.value))
}

const sections = computed(() => {
  const all = [
    { id: 'overview', title: 'Overview', rows: visible(overview.value), total: overview.value.length },
    { id: 'ports', title: 'Ports', rows: visible(ports.value), total: ports.value.length, empty: 'No published ports.' },
    { id: 'env', title: 'Environment', rows: visible(env.value), total: env.value.length, empty: 'No environment variables.' },
    { id: 'labels', title: 'Labels', rows: visible(labels.value), total: labels.value.length, empty: 'No labels.' },
  ]
  // While searching, a section with no hits folds away; idle, every section shows (an empty one
  // says so, which is itself information — "no ports" answers a question).
  return searching.value ? all.filter((s) => s.rows.length) : all
})

const hitCount = computed(() => sections.value.reduce((n, s) => n + s.rows.length, 0) + mounts.value.length)
const nothing = computed(() => searching.value && hitCount.value === 0)

function clear(): void {
  raw.value = ''
}
</script>

<template>
  <div class="flex flex-col text-sm" data-testid="container-details">
    <!-- The find box is sticky so the query stays editable however far the list scrolls. -->
    <div class="sticky top-0 z-10 border-b border-border bg-popover/95 px-4 py-2.5 backdrop-blur">
      <div class="relative">
        <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="raw"
          type="search"
          placeholder="Find in details — an env var, a path, a label…"
          class="h-8 pl-8 pr-24 text-xs"
          data-testid="details-search"
          @keydown.esc="clear"
        />
        <div class="pointer-events-none absolute right-2 top-1/2 flex -translate-y-1/2 items-center gap-1.5 text-[11px] text-muted-foreground">
          <span v-if="searching" data-testid="details-hits">{{ hitCount }} {{ hitCount === 1 ? 'match' : 'matches' }}</span>
          <button v-if="searching" type="button" class="pointer-events-auto rounded p-0.5 hover:bg-muted hover:text-foreground" aria-label="Clear search" @click="clear"><X class="size-3.5" /></button>
        </div>
      </div>
    </div>

    <EmptyState v-if="nothing" title="Nothing matches" :description="`No key, value, path or label contains “${raw.trim()}”.`" class="h-auto py-10" />

    <div v-else class="flex flex-col gap-4 p-4">
      <template v-for="s in sections" :key="s.id">
        <section :data-testid="`details-${s.id}`" class="overflow-hidden rounded-lg border border-border bg-surface-1 shadow-1">
          <header class="flex items-center gap-2 border-b border-border/70 bg-muted/40 px-3 py-1.5">
            <h4 class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">{{ s.title }}</h4>
            <span v-if="s.id !== 'overview' && s.total" class="rounded-full bg-muted px-1.5 text-[10px] tabular-nums text-muted-foreground">{{ searching ? `${s.rows.length} of ${s.total}` : s.total }}</span>
            <span v-if="s.id === 'env' && maskedCount" class="ml-auto inline-flex items-center gap-1 text-[10px] text-muted-foreground" title="Secret-looking values are masked by the agent before they leave the host."><Lock class="size-3" /> {{ maskedCount }} masked</span>
          </header>

          <p v-if="!s.rows.length" class="px-3 py-2.5 text-xs text-muted-foreground">{{ s.empty }}</p>
          <dl v-else class="divide-y divide-border/60">
            <div v-for="r in s.rows" :key="r.key" class="group grid grid-cols-[minmax(8rem,13rem)_minmax(0,1fr)_auto] items-start gap-x-3 px-3 py-1.5 hover:bg-muted/40" data-testid="details-row">
              <dt class="break-all pt-px text-xs text-muted-foreground" :class="r.mono && s.id !== 'overview' && 'font-mono text-[11px]'" :title="r.key"><Highlight :text="r.key" :query="q" /></dt>
              <dd class="min-w-0 text-xs text-foreground" :class="r.mono && 'font-mono text-[11px]'">
                <span v-if="r.masked" class="inline-flex items-center gap-1 italic text-muted-foreground/70" data-testid="masked"><Lock class="size-3" /> masked</span>
                <span v-else-if="r.chips" class="flex flex-wrap gap-1">
                  <Badge v-for="chip in r.chips" :key="chip" variant="outline" class="font-mono"><Highlight :text="chip" :query="q" /></Badge>
                  <span v-if="!r.chips.length" class="text-muted-foreground">—</span>
                </span>
                <RouterLink v-else-if="r.link" :to="r.link" class="break-all text-brand hover:underline"><Highlight :text="r.value" :query="q" /> →</RouterLink>
                <span v-else class="break-all"><Highlight :text="r.value" :query="q" /></span>
                <span v-if="r.note" class="ml-1.5 text-muted-foreground"><Highlight :text="r.note" :query="q" /></span>
              </dd>
              <button
                v-if="r.copy && !r.masked"
                type="button"
                class="-my-0.5 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-muted hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
                :aria-label="`Copy ${r.key}`"
                @click="copy(r.value, r.key)"
              ><Copy class="size-3.5" /></button>
              <span v-else class="size-6" aria-hidden="true" />
            </div>
          </dl>
        </section>
      </template>

      <section v-if="!searching || mounts.length" data-testid="details-mounts" class="overflow-hidden rounded-lg border border-border bg-surface-1 shadow-1">
        <header class="flex items-center gap-2 border-b border-border/70 bg-muted/40 px-3 py-1.5">
          <h4 class="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">Mounts</h4>
          <span v-if="container.mounts.length" class="rounded-full bg-muted px-1.5 text-[10px] tabular-nums text-muted-foreground">{{ searching ? `${mounts.length} of ${container.mounts.length}` : container.mounts.length }}</span>
        </header>
        <p v-if="!mounts.length" class="px-3 py-2.5 text-xs text-muted-foreground">No mounts.</p>
        <ul v-else class="divide-y divide-border/60">
          <!-- One mount per row, source above destination: a bind's host path is routinely 80+
               characters, and side by side it either truncates or forces the drawer wider. -->
          <li v-for="m in mounts" :key="m.destination" class="group grid grid-cols-[minmax(8rem,13rem)_minmax(0,1fr)_auto] items-start gap-x-3 px-3 py-2 hover:bg-muted/40" data-testid="details-row">
            <div class="flex flex-col gap-1 pt-px">
              <span class="text-xs text-muted-foreground"><Highlight :text="m.type" :query="q" /></span>
              <span class="w-fit rounded-sm bg-muted px-1 font-mono text-[10px] text-muted-foreground">{{ m.rw ? 'rw' : 'ro' }}</span>
            </div>
            <div class="min-w-0 font-mono text-[11px]">
              <div class="break-all text-muted-foreground"><Highlight :text="m.source" :query="q" /></div>
              <div class="mt-0.5 flex items-start gap-1 text-foreground"><ArrowRight class="mt-0.5 size-3 shrink-0 text-muted-foreground" /><span class="break-all"><Highlight :text="m.destination" :query="q" /></span></div>
            </div>
            <button type="button" class="-my-0.5 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-muted hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100" :aria-label="`Copy ${m.destination} source`" @click="copy(m.source, 'mount source')"><Copy class="size-3.5" /></button>
          </li>
        </ul>
      </section>
    </div>
  </div>
</template>
