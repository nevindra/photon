<!-- frontend/src/components/metrics/MetricPicker.vue -->
<script setup>
import { ref, computed } from 'vue'
import { Search, ChevronDown, Star } from 'lucide-vue-next'
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover'
import { groupByNamespace, rankMetrics } from '@/lib/metrics/metricNamespaces'
import { cn } from '@/lib/core/utils'

const props = defineProps({
  modelValue: { type: String, default: '' },
  catalog: { type: Array, default: () => [] },
  loading: { type: Boolean, default: false },
  favorites: { type: Array, default: () => [] },
  recent: { type: Array, default: () => [] },
})
const emit = defineEmits(['update:modelValue', 'toggle-favorite'])

const open = ref(false)
const search = ref('')

const byName = (name) => props.catalog.find((m) => m.name === name)
const selected = computed(() => byName(props.modelValue) || null)
const favEntries = computed(() => props.favorites.map(byName).filter(Boolean))
const recentEntries = computed(() => props.recent.map(byName).filter(Boolean).slice(0, 8))

// While searching, flatten to one ranked list; otherwise show namespace groups.
const searching = computed(() => search.value.trim().length > 0)
const ranked = computed(() => rankMetrics(props.catalog, search.value))
const groups = computed(() => groupByNamespace(props.catalog))

function badge(m) {
  return m.unit && m.unit !== '1' ? `${m.type} · ${m.unit}` : m.type
}
function pick(m) {
  emit('update:modelValue', m.name)
  open.value = false
  search.value = ''
}
function star(m) {
  emit('toggle-favorite', m.name)
}
const isFav = (name) => props.favorites.includes(name)
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger as-child>
      <button
        type="button"
        data-testid="metric-picker-trigger"
        :class="cn(
          'flex h-[34px] items-center gap-2 rounded-lg border border-border bg-background px-2.5 text-[13px] font-medium transition-colors hover:border-foreground/40 data-[state=open]:border-foreground/40',
          'min-w-[220px] max-w-[360px]',
        )"
      >
        <Search class="size-3.5 shrink-0 text-muted-foreground" />
        <span class="truncate font-mono" :class="modelValue ? 'text-foreground' : 'font-sans font-medium text-muted-foreground'">
          {{ modelValue || 'Select a metric…' }}
        </span>
        <span
          v-if="selected"
          class="shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[9px] font-normal uppercase tracking-[0.08em] text-muted-foreground"
        >{{ badge(selected) }}</span>
        <ChevronDown class="ml-auto size-3.5 shrink-0 text-muted-foreground" />
      </button>
    </PopoverTrigger>
    <PopoverContent align="start" class="w-[380px] p-0">
      <div class="border-b border-border p-2">
        <input
          data-testid="metric-picker-search"
          v-model="search"
          autofocus
          placeholder="Filter metrics…"
          class="w-full bg-transparent px-1 text-[13px] outline-none placeholder:text-muted-foreground"
        />
      </div>
      <div class="max-h-[340px] overflow-auto py-1">
        <!-- flat ranked results while searching -->
        <template v-if="searching">
          <button
            v-for="m in ranked" :key="m.name" type="button" data-testid="metric-option"
            class="flex w-full items-center gap-2 px-3 py-1.5 text-left font-mono text-[13px] transition-colors hover:bg-muted"
            @click="pick(m)"
          >
            <span role="button" tabindex="0" data-testid="metric-star" class="shrink-0 cursor-pointer" @click.stop="star(m)" @keydown.enter.stop="star(m)">
              <Star class="size-3" :class="isFav(m.name) ? 'fill-brand text-brand' : 'text-muted-foreground/50'" />
            </span>
            <span class="truncate text-foreground">{{ m.name }}</span>
            <span class="ml-auto shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 text-[9px] uppercase tracking-[0.08em] text-muted-foreground">{{ badge(m) }}</span>
          </button>
          <div v-if="!ranked.length" class="px-3 py-6 text-center text-[12px] text-muted-foreground">
            {{ loading ? 'Loading…' : 'No metrics match' }}
          </div>
        </template>

        <!-- favorites / recent / namespace groups when not searching -->
        <template v-else>
          <section v-if="favEntries.length">
            <div class="px-3 pb-1 pt-1.5 text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">★ Favorites</div>
            <button
              v-for="m in favEntries" :key="'fav-' + m.name" type="button" data-testid="metric-option"
              class="flex w-full items-center gap-2 px-3 py-1.5 text-left font-mono text-[13px] transition-colors hover:bg-muted"
              @click="pick(m)"
            >
              <span role="button" tabindex="0" data-testid="metric-star" class="shrink-0 cursor-pointer" @click.stop="star(m)" @keydown.enter.stop="star(m)">
                <Star class="size-3 fill-brand text-brand" />
              </span>
              <span class="truncate text-foreground">{{ m.name }}</span>
              <span class="ml-auto shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 text-[9px] uppercase tracking-[0.08em] text-muted-foreground">{{ badge(m) }}</span>
            </button>
          </section>

          <section v-if="recentEntries.length">
            <div class="px-3 pb-1 pt-1.5 text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">↺ Recent</div>
            <button
              v-for="m in recentEntries" :key="'rec-' + m.name" type="button" data-testid="metric-option"
              class="flex w-full items-center gap-2 px-3 py-1.5 text-left font-mono text-[13px] transition-colors hover:bg-muted"
              @click="pick(m)"
            >
              <span class="truncate text-foreground">{{ m.name }}</span>
              <span class="ml-auto shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 text-[9px] uppercase tracking-[0.08em] text-muted-foreground">{{ badge(m) }}</span>
            </button>
          </section>

          <section v-for="g in groups" :key="'ns-' + (g.name || 'other')">
            <div class="px-3 pb-1 pt-1.5 text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
              {{ g.name || 'Other' }} <span class="text-muted-foreground/60">({{ g.metrics.length }})</span>
            </div>
            <button
              v-for="m in g.metrics" :key="m.name" type="button" data-testid="metric-option"
              class="flex w-full items-center gap-2 px-3 py-1.5 text-left font-mono text-[13px] transition-colors hover:bg-muted"
              @click="pick(m)"
            >
              <span role="button" tabindex="0" data-testid="metric-star" class="shrink-0 cursor-pointer" @click.stop="star(m)" @keydown.enter.stop="star(m)">
                <Star class="size-3" :class="isFav(m.name) ? 'fill-brand text-brand' : 'text-muted-foreground/40'" />
              </span>
              <span class="truncate text-foreground">{{ m.name }}</span>
              <span class="ml-auto shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 text-[9px] uppercase tracking-[0.08em] text-muted-foreground">{{ badge(m) }}</span>
            </button>
          </section>

          <div v-if="!catalog.length" class="px-3 py-6 text-center text-[12px] text-muted-foreground">
            {{ loading ? 'Loading…' : 'No metrics' }}
          </div>
        </template>
      </div>
    </PopoverContent>
  </Popover>
</template>
