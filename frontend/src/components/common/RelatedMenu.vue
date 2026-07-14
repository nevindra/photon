<script setup lang="ts">
// "Related ▾" dropdown: renders the typed cross-signal destination list for an entity (from
// useCorrelate's relatedFor) and navigates via correlate() so every hop carries the active time
// window + scope. Phase-2 flagship destinations render disabled with a "SOON" badge.
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { ChevronDown } from 'lucide-vue-next'
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu'
import { relatedFor, correlate, type RelatedDestination } from '@/lib/core/useCorrelate'

const props = withDefaults(
  defineProps<{
    entity: { kind: string; fields?: Record<string, string | undefined> }
    label?: string
  }>(),
  { label: 'Related' },
)

const router = useRouter()
const items = computed(() => relatedFor(props.entity))
const primary = computed(() => items.value.filter((i) => i.phase !== 2))
const flagship = computed(() => items.value.filter((i) => i.phase === 2))

function go(item: RelatedDestination): void {
  if (item.phase === 2) return
  router.push(correlate(item.dest))
}
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger
      data-testid="related-trigger"
      class="inline-flex items-center gap-1 rounded-md border border-brand/30 bg-brand-soft px-2 py-1 text-xs text-brand"
    >
      {{ label }} <ChevronDown class="size-3.5" />
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="w-64">
      <DropdownMenuLabel class="text-[10px] uppercase tracking-wider text-muted-foreground">Jump to</DropdownMenuLabel>
      <DropdownMenuItem
        v-for="item in primary"
        :key="item.id"
        :data-related-id="item.id"
        @select="go(item)"
      >
        {{ item.label }}
      </DropdownMenuItem>
      <template v-if="flagship.length">
        <DropdownMenuSeparator />
        <DropdownMenuItem
          v-for="item in flagship"
          :key="item.id"
          :data-related-id="item.id"
          disabled
          class="text-brand"
        >
          {{ item.label }}
          <span class="ml-auto rounded bg-brand px-1.5 text-[9px] font-bold text-brand-foreground">SOON</span>
        </DropdownMenuItem>
      </template>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
