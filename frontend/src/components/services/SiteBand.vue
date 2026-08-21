<script setup lang="ts">
// The Site band across the top of the index: an All-sites card, then one card per registered Site,
// worst first. It is the page's Site filter — there is no separate picker, because a picker that
// showed no health would make you choose blind.
//
// Renders nothing on a Site install: `sites` is the tenant registry, and a Site install has none.
import SiteCard from './SiteCard.vue'
import type { SiteStat } from '@/lib/services/servicesOverview'

defineProps<{ sites: SiteStat[]; active?: string | null }>()
const emit = defineEmits<{ select: [key: string | null] }>()

// Clicking the active Site clears the filter, so the band never traps you inside one Site.
function onSelect(key: string | null, isActive: boolean): void {
  emit('select', isActive ? null : key)
}
</script>

<template>
  <section v-if="sites.length" data-testid="site-band" aria-label="Sites">
    <div class="flex gap-3 overflow-x-auto pb-1">
      <SiteCard
        v-for="s in sites"
        :key="s.key ?? '__all__'"
        :site="s"
        :active="(active ?? null) === s.key"
        @select="onSelect($event, (active ?? null) === s.key)"
      />
    </div>
  </section>
</template>
