<script setup>
// A titled block: an uppercase section label, an optional active-count badge, a quiet Clear ✕
// that surfaces on hover (only when the section is active), and the loading / fetching / empty
// states — with the rows fed in through the default slot. Used for every pinned section
// (Services/Severity, Service/Status/Kind) and reused as the shell for the catalog's own header.
// Purely presentational: the parent adapter owns the data and the `clear` handler.
import { Skeleton } from '@/components/ui/skeleton'
import { X } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

defineProps({
  // Uppercase section label, e.g. "Services".
  label: { type: String, default: '' },
  // Whether the field has any constraints — shows the Clear ✕ on hover.
  active: { type: Boolean, default: false },
  // Optional active-count badge; shown when > 0.
  count: { type: Number, default: null },
  // Before ANY data has loaded → render the 3-line skeleton shimmer instead of the rows.
  loading: { type: Boolean, default: false },
  // Refetching over already-loaded data → dim the list (kept populated, not skeleton'd).
  fetching: { type: Boolean, default: false },
  // No rows to show → render the muted empty line.
  empty: { type: Boolean, default: false },
  emptyText: { type: String, default: 'No values' },
  // `data-test` for the Clear button (e.g. `fr-clear-service`, `qf-clear-status`).
  clearDataTest: { type: String, default: undefined },
  // Optional `data-test` for the skeleton container (e.g. `qf-service-skeleton`), so an adapter
  // can preserve its existing loading-state hook even though this component owns the branch.
  loadingDataTest: { type: String, default: undefined },
})

const emit = defineEmits(['clear'])
</script>

<template>
  <section class="group/facet-section px-2 py-3">
    <div class="mb-2 flex items-center gap-2 px-1.5">
      <span class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">{{ label }}</span>
      <span
        v-if="count"
        class="rounded-full bg-muted px-1.5 text-[10px] font-medium leading-4 tabular-nums text-muted-foreground"
      >
        {{ count }}
      </span>
      <button
        v-if="active"
        type="button"
        :data-test="clearDataTest"
        :aria-label="`Clear ${label}`"
        class="ml-auto grid size-[18px] place-items-center rounded-sm text-muted-foreground opacity-0 transition-opacity motion-reduce:transition-none hover:bg-accent hover:text-accent-foreground focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring group-hover/facet-section:opacity-100"
        @click="emit('clear')"
      >
        <X class="size-3" />
      </button>
    </div>

    <div v-if="loading" :data-test="loadingDataTest" class="flex flex-col gap-1 px-1.5 py-1" aria-hidden="true">
      <Skeleton v-for="i in 3" :key="i" class="h-3.5" :style="{ width: `${70 - i * 12}%` }" />
    </div>
    <p v-else-if="empty" class="px-1.5 py-1 text-[11px] text-muted-foreground">{{ emptyText }}</p>
    <div
      v-else
      :class="
        cn(
          'flex flex-col gap-0.5 transition-opacity duration-[var(--motion-base)] motion-reduce:transition-none',
          fetching && 'opacity-60',
        )
      "
    >
      <slot />
    </div>
  </section>
</template>
