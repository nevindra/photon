<script setup lang="ts">
// Text with every occurrence of the find-in-details query marked. The mark is the brand tint,
// not severity yellow: a hit is "the thing you asked for", not a warning.
import { computed } from 'vue'
import { splitHits } from '@/lib/deploy/detailsSearch'

const props = defineProps<{ text: string; query: string }>()
const segments = computed(() => splitHits(props.text, props.query))
</script>

<template>
  <template v-for="(s, i) in segments" :key="i"><mark v-if="s.hit" class="rounded-[3px] bg-brand-soft px-px text-brand-strong">{{ s.text }}</mark><template v-else>{{ s.text }}</template></template>
</template>
