<script setup>
import { ref, computed } from 'vue'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Checkbox } from '@/components/ui/checkbox'
import { buttonVariants } from '@/components/ui/button'
import { cn } from '@/lib/core/utils'

// Shared column-customization popover. Self-contained: it owns the "Columns" trigger + popover,
// so callers just drop it in with the field list and current selection.
//   available  [{ key, label, group }]  every toggleable column, pre-grouped by the caller
//   selected   Set<string>              the keys currently shown
// Emits toggle(key) when a row is clicked; the parent owns the selection state.
const props = defineProps({
  available: { type: Array, default: () => [] },
  selected: { type: Set, default: () => new Set() },
})
const emit = defineEmits(['toggle'])

// A filter input is only worth its screen space past a handful of fields.
const filter = ref('')
const showFilter = computed(() => props.available.length > 8)

const filtered = computed(() => {
  const q = filter.value.trim().toLowerCase()
  if (!q) return props.available
  return props.available.filter(
    (f) => (f.label ?? f.key).toLowerCase().includes(q) || f.key.toLowerCase().includes(q),
  )
})

// Group in first-seen order so headers follow the caller's ordering (built-ins before attributes).
const groups = computed(() => {
  const map = new Map()
  for (const item of filtered.value) {
    const g = item.group ?? ''
    if (!map.has(g)) map.set(g, [])
    map.get(g).push(item)
  }
  return [...map.entries()].map(([group, items]) => ({ group, items }))
})
</script>

<template>
  <Popover>
    <PopoverTrigger :class="cn(buttonVariants({ variant: 'outline', size: 'sm' }), 'font-mono text-xs')">
      Columns
    </PopoverTrigger>
    <PopoverContent align="end" class="w-56 p-1">
      <div class="flex flex-col gap-0.5 text-xs">
        <input
          v-if="showFilter"
          v-model="filter"
          data-test="col-filter"
          type="text"
          placeholder="Filter fields…"
          class="mb-1 w-full rounded-sm border border-border bg-transparent px-2 py-1 font-mono text-xs outline-none focus-visible:ring-1 focus-visible:ring-ring"
        />
        <template v-for="g in groups" :key="g.group">
          <div
            v-if="g.group"
            class="px-2 pb-0.5 pt-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground"
          >
            {{ g.group }}
          </div>
          <div
            v-for="item in g.items"
            :key="item.key"
            :data-test="`col-toggle-${item.key}`"
            class="flex cursor-pointer items-center gap-2 px-2 py-1 text-left hover:bg-accent"
            @click="emit('toggle', item.key)"
          >
            <Checkbox :model-value="selected.has(item.key)" class="pointer-events-none" />
            <span class="truncate font-mono">{{ item.label ?? item.key }}</span>
          </div>
        </template>
        <div
          v-if="!groups.length"
          class="px-2 py-1 font-mono text-muted-foreground"
        >
          No fields
        </div>
      </div>
    </PopoverContent>
  </Popover>
</template>
