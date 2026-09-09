<script setup lang="ts">
// A template's icon: an emoji from the manifest, an image URL from a registry, or the name's
// initial on a soft tile when there is neither.
import { computed, ref, watch } from 'vue'
import { cn } from '@/lib/core/utils'

const props = defineProps<{ icon: string; name: string; size?: 'sm' | 'md' | 'lg' }>()
// A registry icon that fails to load (offline site, blocked host) falls back to the initial.
const broken = ref(false)
const loaded = ref(false)
watch(() => props.icon, () => { broken.value = false; loaded.value = false })
const isUrl = computed(() => /^https?:\/\//.test(props.icon) && !broken.value)
const isEmoji = computed(() => !!props.icon && !isUrl.value && props.icon.length <= 4)
const box = computed(() => (props.size === 'lg' ? 'size-14 text-3xl rounded-xl' : props.size === 'sm' ? 'size-7 text-sm rounded-md' : 'size-10 text-xl rounded-lg'))
</script>

<template>
  <span :class="cn('flex shrink-0 items-center justify-center overflow-hidden bg-brand-soft text-brand', box)" aria-hidden="true">
    <!-- The initial shows until a registry image has actually loaded, so a slow or blocked host
         never leaves an empty tile. -->
    <img v-if="isUrl" :src="icon" alt="" class="size-full object-contain p-1" :class="loaded ? '' : 'hidden'" @load="loaded = true" @error="broken = true" />
    <span v-if="isEmoji">{{ icon }}</span>
    <span v-else-if="!isUrl || !loaded" class="font-semibold">{{ name.slice(0, 1).toUpperCase() }}</span>
  </span>
</template>
