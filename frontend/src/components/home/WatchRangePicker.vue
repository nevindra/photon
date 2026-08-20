<script setup lang="ts">
// The Attention Board's own time control. Deliberately a Segmented and not the global
// TimeRangePicker: the choice is four bounded values, it is a toggle rather than a menu, and one
// click beats two on a page people leave open. The cap IS the design — see
// docs/adr/0001-landing-page-is-an-attention-board.md.
import { Eye } from 'lucide-vue-next'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { WATCH_RANGES, watchRange, setWatchRange } from '@/lib/home/watchRange'
</script>

<template>
  <div class="flex items-center gap-2">
    <span class="hidden items-center gap-1.5 text-[11px] font-medium text-muted-foreground sm:inline-flex">
      <Eye class="size-3.5" />
      Watching
    </span>
    <Segmented
      type="single"
      :model-value="watchRange"
      data-testid="watch-range"
      aria-label="Watch range"
      @update:model-value="(v: unknown) => typeof v === 'string' && setWatchRange(v)"
    >
      <SegmentedItem v-for="r in WATCH_RANGES" :key="r" :value="r" :aria-label="`Last ${r}`">
        {{ r }}
      </SegmentedItem>
    </Segmented>
  </div>
</template>
