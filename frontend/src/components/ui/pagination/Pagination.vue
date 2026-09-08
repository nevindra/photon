<script setup lang="ts">
// The strip under a paged table: where you are in the whole ("11–20 of 37"), how many rows per
// page, and Previous / Next. It says nothing when there is one page and no size to pick — the
// parent decides whether to mount it at all. Every number comes from the parent's `paginate()`
// result, so the strip never disagrees with the rows above it.
import { ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { SelectMenu } from '@/components/ui/select-menu'

const props = withDefaults(
  defineProps<{
    /** 1-based current page, already clamped. */
    page: number
    pages: number
    /** 1-based inclusive bounds of the rows on screen (both 0 when there are none). */
    from: number
    to: number
    total: number
    pageSize: number
    /** The sizes offered; omit to hide the picker. */
    pageSizes?: readonly number[]
    /** What is being counted — "users", "rows". */
    noun?: string
  }>(),
  { pageSizes: () => [], noun: 'rows' },
)
const emit = defineEmits<{ 'update:page': [page: number]; 'update:pageSize': [size: number] }>()

const sizeOptions = () => props.pageSizes.map((n) => ({ value: n, label: String(n) }))
function go(p: number): void {
  const next = Math.min(Math.max(1, p), props.pages)
  if (next !== props.page) emit('update:page', next)
}
</script>

<template>
  <nav class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 text-xs text-muted-foreground" aria-label="Pagination" data-testid="pagination">
    <p class="tabular-nums" data-testid="pagination-range">
      <template v-if="total === 0">No {{ noun }}</template>
      <template v-else>
        <span class="font-medium text-foreground">{{ from }}–{{ to }}</span> of {{ total }} {{ noun }}
      </template>
    </p>
    <div class="flex items-center gap-3">
      <SelectMenu
        v-if="pageSizes.length > 1"
        :model-value="pageSize"
        :options="sizeOptions()"
        prefix="Per page:"
        content-class="w-28"
        align="end"
        aria-label="Rows per page"
        data-testid="pagination-size"
        @update:model-value="(v: unknown) => emit('update:pageSize', Number(v))"
      />
      <div class="flex items-center gap-1">
        <Button variant="outline" size="icon" class="size-7" :disabled="page <= 1" aria-label="Previous page" data-testid="pagination-prev" @click="go(page - 1)">
          <ChevronLeft class="size-3.5" />
        </Button>
        <span class="min-w-[5.5rem] text-center tabular-nums" data-testid="pagination-page">Page {{ page }} of {{ pages }}</span>
        <Button variant="outline" size="icon" class="size-7" :disabled="page >= pages" aria-label="Next page" data-testid="pagination-next" @click="go(page + 1)">
          <ChevronRight class="size-3.5" />
        </Button>
      </div>
    </div>
  </nav>
</template>
