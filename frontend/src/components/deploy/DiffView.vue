<script setup lang="ts">
// A line diff of two rendered compose files: what changes on the host if this ships. Long
// unchanged runs collapse to a "… n lines" marker; the summary counts sit in the header.
import { computed } from 'vue'
import { lineDiff, diffStats, collapseDiff } from '@/lib/deploy/diff'

const props = defineProps<{ before: string; after: string; title?: string }>()
const lines = computed(() => lineDiff(props.before, props.after))
const stats = computed(() => diffStats(lines.value))
const rows = computed(() => collapseDiff(lines.value, 3))
</script>

<template>
  <div class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="diff-view">
    <header class="flex items-center gap-3 border-b border-border px-3 py-2 text-xs">
      <span class="font-medium text-foreground">{{ title ?? 'Changes' }}</span>
      <span v-if="stats.added || stats.removed" class="font-mono tabular-nums"><span class="text-success">+{{ stats.added }}</span> <span class="text-sev-error">−{{ stats.removed }}</span></span>
      <span v-else class="text-muted-foreground">no changes</span>
    </header>
    <pre class="max-h-80 overflow-auto p-0 font-mono text-[11.5px] leading-5"><template v-for="(l, i) in rows" :key="i"><div v-if="l.type === 'skip'" class="bg-muted/40 px-3 text-muted-foreground/70">… {{ l.count }} unchanged lines</div><div v-else :class="['flex px-3', l.type === 'add' ? 'bg-success-soft text-success' : l.type === 'del' ? 'bg-sev-error-soft text-sev-error line-through decoration-sev-error/40' : 'text-foreground']" :data-diff="l.type"><span class="w-4 shrink-0 select-none opacity-60">{{ l.type === 'add' ? '+' : l.type === 'del' ? '−' : ' ' }}</span><span class="whitespace-pre">{{ l.text }}</span></div></template></pre>
  </div>
</template>
