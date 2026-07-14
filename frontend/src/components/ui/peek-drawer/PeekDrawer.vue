<script setup>
import { computed, onUnmounted, watch } from 'vue'
import { Sheet, SheetContent, SheetHeader } from '@/components/ui/sheet'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Kbd } from '@/components/ui/kbd'
import { ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  open: { type: Boolean, default: false },
  hasContent: { type: Boolean, default: true },
  index: { type: Number, default: -1 },
  total: { type: Number, default: 0 },
  width: { type: [Number, String], default: 480 },
})

const emit = defineEmits(['update:open', 'prev', 'next', 'shortcut'])

// Apply width via inline style rather than a dynamic `w-[Npx]` Tailwind class: Tailwind's JIT
// scanner does raw-text extraction over source, so it would never emit CSS for a width computed
// from a numeric prop. Inline style is JIT-proof and beats sheetVariants' base `w-3/4
// sm:max-w-sm` on specificity, so the drawer renders at exactly `width` px.
const widthStyle = computed(() => ({
  width: `${Number(props.width)}px`,
  maxWidth: `${Number(props.width)}px`,
}))

// Reka's Sheet traps focus inside the teleported content, so a component-local keydown listener
// won't receive keys typed anywhere else on the page — bind on `window` instead, and keep it
// scoped to "while open" so a closed/backgrounded drawer can't steal navigation from nowhere.
function onKey(e) {
  if (e.metaKey || e.ctrlKey || e.altKey) return
  const t = e.target
  if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
  if (e.key === 'j' || e.key === 'ArrowDown') {
    e.preventDefault()
    emit('next')
  } else if (e.key === 'k' || e.key === 'ArrowUp') {
    e.preventDefault()
    emit('prev')
  } else {
    emit('shortcut', e)
  }
}

watch(
  () => props.open,
  (o) => {
    if (o) window.addEventListener('keydown', onKey)
    else window.removeEventListener('keydown', onKey)
  },
  { immediate: true },
)
onUnmounted(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <Sheet :open="open" @update:open="$emit('update:open', $event)">
    <SheetContent
      v-if="hasContent"
      side="right"
      :class="cn('flex flex-col gap-0')"
      :style="widthStyle"
    >
      <SheetHeader class="shrink-0 border-b border-border px-6 py-4 text-left">
        <!-- Pair each key hint with its arrow so position matches motion: left/back = k (prev),
             right/forward = j (next). Reads `‹ k   3 / 128   j ›`. -->
        <div v-if="total > 1" class="mb-2 flex items-center gap-2">
          <span class="flex items-center gap-1">
            <button
              type="button"
              data-testid="peek-drawer-prev"
              class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
              :disabled="index <= 0"
              @click="$emit('prev')"
            >
              <ChevronLeft class="h-4 w-4" />
            </button>
            <Kbd class="text-muted-foreground">k</Kbd>
          </span>
          <span class="font-mono text-xs text-muted-foreground">{{ index + 1 }} / {{ total }}</span>
          <span class="flex items-center gap-1">
            <Kbd class="text-muted-foreground">j</Kbd>
            <button
              type="button"
              data-testid="peek-drawer-next"
              class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
              :disabled="index >= total - 1"
              @click="$emit('next')"
            >
              <ChevronRight class="h-4 w-4" />
            </button>
          </span>
        </div>
        <slot name="header" />
      </SheetHeader>
      <ScrollArea class="min-h-0 flex-1">
        <slot />
      </ScrollArea>
    </SheetContent>
  </Sheet>
</template>
