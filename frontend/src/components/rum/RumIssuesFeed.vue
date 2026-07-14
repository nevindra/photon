<script setup lang="ts">
// "Live issues" feed for the RUM executive summary — the most frequent JS error issues across all
// apps (see `topIssues`), each tagged with its app and drilling into that app's error issues on
// click. The `GET /api/rum/errors` issue shape carries only type/message/count/sessions (no route
// or last-seen), so this shows exactly that — no fabricated "N ago".
import { formatNumber } from '@/lib/core/format'
import type { Issue } from '@/lib/rum/rumSummary'

defineProps<{ issues: Issue[] }>()
const emit = defineEmits<{ open: [app: string] }>()
</script>

<template>
  <ul class="flex flex-col">
    <li
      v-for="issue in issues"
      :key="issue.app + ':' + issue.fingerprint"
      data-testid="rum-issue"
      role="button"
      tabindex="0"
      class="flex cursor-pointer items-start gap-3 border-b border-border/50 py-2.5 transition-colors last:border-0 hover:bg-muted/40 focus-visible:bg-muted/40 focus-visible:outline-none"
      @click="emit('open', issue.app)"
      @keydown.enter="emit('open', issue.app)"
    >
      <div class="min-w-0 flex-1">
        <span class="text-sm font-semibold text-sev-error">{{ issue.exception_type }}</span>
        <span class="mt-0.5 block truncate text-xs text-foreground">{{ issue.message }}</span>
        <div class="mt-1.5 flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
          <span class="rounded bg-muted px-1.5 py-0.5 font-mono">{{ issue.app }}</span>
          <span>{{ formatNumber(issue.sessions) }} sessions</span>
        </div>
      </div>
      <span class="shrink-0 pt-0.5 font-mono text-sm font-semibold tabular-nums text-sev-error">
        {{ formatNumber(issue.count) }}
      </span>
    </li>
  </ul>
</template>
