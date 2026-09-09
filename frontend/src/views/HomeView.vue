<script setup lang="ts">
// `/home` — the Attention Board. The landing page answers ONE question: is anything wrong, and
// where? Every watched thing is judged into a single ranked Attention Level (Down / Critical /
// Warning / OK, plus Unknown for the absence of a judgment), and the page leads with whatever
// failed the judgment. See docs/adr/0001-landing-page-is-an-attention-board.md for why, and
// CONTEXT.md for the vocabulary (Subject, Silence, Watch Range, Spread, Unattributed Incident).
//
// This view is the same code on Central and on a Site install. The difference is only in the data:
// on Central every Subject carries a Site and a fifth World (Sites) appears; on a Site install
// none do. There is no role check anywhere — scoping falls out of what the data already is.
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { Check, ArrowRight, X } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import WatchRangePicker from '@/components/home/WatchRangePicker.vue'
import WorldRollupCard from '@/components/home/WorldRollupCard.vue'
import SubjectRow from '@/components/home/SubjectRow.vue'
import VitalSigns from '@/components/home/VitalSigns.vue'
import UnattributedBanner from '@/components/home/UnattributedBanner.vue'
import AttentionLegend from '@/components/home/AttentionLegend.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { api } from '@/lib/core/api'
import { tenant as activeTenant } from '@/lib/core/context'
import { correlate } from '@/lib/core/useCorrelate'
import { useUrlParam } from '@/lib/core/useUrlState'
import { boardWorlds, type Subject, type WorldKey } from '@/lib/home/attention'
import { useAttentionBoard } from '@/lib/home/useAttentionBoard'
import { applyWatchRangeToWindow, useWatchRange, watchNowMs, watchRange } from '@/lib/home/watchRange'

const router = useRouter()

useWatchRange()
// Destructured so the template sees top-level refs (Vue only auto-unwraps those, not refs
// reached through a plain object).
const { subjects, rollups, unattributed, totalWatched, showSites, loading, attention: attentionFor, vitals: vitalsFor } =
  useAttentionBoard()

// The roll-up cards ARE the filter — click one to narrow the list and the vital signs, click it
// again to release. Kept in the URL so a narrowed board is shareable.
const worldParam = ref<string | null>(null)
useUrlParam('world', worldParam, { valid: (v) => boardWorlds({ sites: true, deploy: true }).some((w) => w.key === v) })

// A world this install does not show (`?world=sites` on a non-federated install, or `?world=ops`
// under a tenant lens) filters to nothing — fall back to the whole board rather than an empty page.
const activeWorld = computed(() => rollups.value.find((r) => r.world.key === worldParam.value) ?? null)
const world = computed<WorldKey | null>(() => activeWorld.value?.world.key ?? null)
const attention = attentionFor(world)
const vitals = vitalsFor(world)

function toggleWorld(key: string): void {
  worldParam.value = worldParam.value === key ? null : key
}

// Every hop hands the Watch Range to the app-wide Window FIRST, so the page you land on shows the
// same slice of time — `correlate()` reads the global context.
function go(path: string): void {
  applyWatchRangeToWindow()
  router.push(correlate({ path }))
}
function openSubject(subject: Subject): void {
  go(subject.dest)
}

const crumb = computed(() => (activeTenant.value ? `${activeTenant.value} · Home` : 'Home · Overview'))
const attentionNote = computed(() => {
  const shown = attention.value.length
  const w = activeWorld.value
  if (w) return `${shown} of ${subjects.value.filter((s) => s.world === w.world.key).length} · filtered to ${w.world.name}`
  return shown ? `${shown} subject${shown > 1 ? 's' : ''} · worst first` : 'across every world'
})
const allClearNote = computed(() => {
  const w = activeWorld.value
  if (w) return `All ${w.total} ${w.world.noun} inside their thresholds for the last ${watchRange.value}.`
  return `${totalWatched.value} subjects watched, all inside their thresholds for the last ${watchRange.value}.`
})
</script>

<template>
  <AppShell active="home" :mock="api.mock" :crumb="crumb" :time-picker="false">
    <template #actions><WatchRangePicker /></template>

    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="home">
      <UnattributedBanner
        v-if="unattributed.length"
        :items="unattributed"
        @open="go('/alerts')"
      />

      <!-- Every world, always fully populated — the proof that N subjects are watched, which is
           what makes an empty attention list believable. -->
      <section>
        <header class="mb-2 flex items-baseline gap-2.5">
          <h2 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">All components</h2>
          <span class="text-[11px] text-muted-foreground/80">
            {{ totalWatched }} subjects watched · click a card to filter
          </span>
        </header>
        <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4" data-testid="world-rollup">
          <WorldRollupCard
            v-for="rollup in rollups"
            :key="rollup.world.key"
            :rollup="rollup"
            :active="world === rollup.world.key"
            @toggle="toggleWorld"
          />
        </div>
      </section>

      <section>
        <header class="mb-2 flex items-baseline gap-2.5">
          <h2 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">Needs attention</h2>
          <span class="text-[11px] text-muted-foreground/80">{{ attentionNote }}</span>
          <span v-if="activeWorld" class="ml-auto flex items-center gap-3">
            <button
              type="button"
              class="inline-flex items-center gap-1 rounded-sm text-xs font-medium text-brand hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              @click="worldParam = null"
            >
              <X class="size-3" />
              clear filter
            </button>
            <button
              type="button"
              class="inline-flex items-center gap-1 rounded-sm text-xs font-medium text-brand hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              @click="go(activeWorld.world.dest)"
            >
              Open {{ activeWorld.world.name }}
              <ArrowRight class="size-3" />
            </button>
          </span>
        </header>

        <div
          v-if="attention.length"
          class="overflow-hidden rounded-xl border border-border bg-card shadow-1"
          data-testid="attention-list"
        >
          <SubjectRow
            v-for="subject in attention"
            :key="subject.id"
            :subject="subject"
            :now-ms="watchNowMs"
            :show-sites="showSites"
            @open="openSubject"
          />
        </div>
        <div
          v-else-if="loading"
          class="flex flex-col gap-2 rounded-xl border border-border bg-card p-4 shadow-1"
        >
          <Skeleton v-for="i in 3" :key="i" class="h-10 w-full" />
        </div>
        <div
          v-else
          class="rounded-xl border border-border bg-card px-5 py-10 text-center shadow-1"
          data-testid="all-clear"
        >
          <span class="mx-auto mb-3 flex size-10 items-center justify-center rounded-full bg-success-soft text-success">
            <Check class="size-5" />
          </span>
          <p class="text-sm font-semibold text-foreground">Nothing needs attention</p>
          <p class="mt-1 text-xs text-muted-foreground">{{ allClearNote }}</p>
        </div>
      </section>

      <!-- Thresholds can all hold while the fleet quietly loses most of its traffic. This is where
           that shows up. -->
      <section>
        <header class="mb-2 flex items-baseline gap-2.5">
          <h2 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Vital signs{{ activeWorld ? ` · ${activeWorld.world.name}` : '' }}
          </h2>
          <span class="text-[11px] text-muted-foreground/80">
            last {{ watchRange }} vs the {{ watchRange }} before it
          </span>
        </header>
        <VitalSigns :vitals="vitals" />
      </section>

      <AttentionLegend />
    </main>
  </AppShell>
</template>
