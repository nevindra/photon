<script setup lang="ts">
// One Subject that needs attention. The row answers "where is it wrong?" in reading order: how bad
// (rail + pill), what (name + kind), where (Site badge / Spread on Central), who asked to be told
// (incident badge), why (the reason line, keeping each kind's own word verbatim), and how long.
import { computed } from 'vue'
import { TriangleAlert } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { StatusPill } from '@/components/ui/status-pill'
import { LEVEL_META, KIND_LABEL, compactAge, type Subject } from '@/lib/home/attention'

const props = defineProps<{
  subject: Subject
  nowMs: number
  /** Central shows where a Subject lives; a Site install has one place and never says so. */
  showSites?: boolean
}>()

defineEmits<{ (e: 'open', subject: Subject): void }>()

const meta = computed(() => LEVEL_META[props.subject.level])
const age = computed(() => (props.subject.sinceMs == null ? null : compactAge(props.nowMs - props.subject.sinceMs)))
const since = computed(() =>
  props.subject.sinceMs == null
    ? null
    : new Date(props.subject.sinceMs).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
)
// Unhealthy in every Site it runs in points at the software; in one, at that Site.
const spreadEverywhere = computed(() => !!props.subject.spread && props.subject.spread.bad >= props.subject.spread.total)
</script>

<template>
  <button
    type="button"
    class="grid w-full grid-cols-[3px_84px_minmax(0,1fr)_auto] items-center border-b border-border/60 text-left transition-colors duration-100 last:border-b-0 hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    :data-level="subject.level"
    :data-subject="subject.id"
    @click="$emit('open', subject)"
  >
    <span class="h-full self-stretch" :class="meta.rail" />

    <span class="py-3 pl-3">
      <StatusPill :tone="meta.tone" :solid="meta.solid" class="text-[10px] font-bold uppercase tracking-wide">
        {{ meta.label }}
      </StatusPill>
    </span>

    <span class="min-w-0 py-3">
      <span class="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span class="truncate font-mono text-sm font-semibold text-foreground">{{ subject.name }}</span>
        <span class="text-[11px] text-muted-foreground">{{ KIND_LABEL[subject.kind] }}</span>

        <template v-if="showSites">
          <Badge v-if="subject.spread" :variant="spreadEverywhere ? 'error' : 'secondary'" class="tabular-nums">
            {{ subject.spread.bad }} of {{ subject.spread.total }} sites
          </Badge>
          <Badge v-if="subject.sites.length" variant="brand">{{ subject.sites.join(' · ') }}</Badge>
          <Badge v-else-if="subject.local && !subject.spread" variant="secondary">central</Badge>
        </template>

        <Badge v-if="subject.incidents" variant="error">
          <TriangleAlert />
          {{ subject.incidents }} incident{{ subject.incidents > 1 ? 's' : '' }}
        </Badge>
        <Badge v-else variant="outline" class="text-muted-foreground">no alert rule</Badge>
      </span>

      <span v-if="subject.reasons.length" class="mt-1 block truncate text-xs text-muted-foreground">
        <template v-for="(reason, i) in subject.reasons" :key="i">
          <span v-if="i" class="px-1.5 text-muted-foreground/60">·</span>
          <span>{{ reason.label }}</span>
          <span v-if="reason.value" class="px-1 font-medium tabular-nums text-foreground">{{ reason.value }}</span>
          <span v-if="reason.note">({{ reason.note }})</span>
        </template>
      </span>
    </span>

    <span class="py-3 pl-3 pr-4 text-right">
      <template v-if="age">
        <span class="block text-xs font-semibold tabular-nums text-foreground">{{ age }}</span>
        <span class="block text-[10px] tabular-nums text-muted-foreground">since {{ since }}</span>
      </template>
    </span>
  </button>
</template>
