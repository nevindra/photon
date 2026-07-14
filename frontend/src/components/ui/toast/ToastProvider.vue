<script setup lang="ts">
import { ToastProvider, type ToastProviderProps, useForwardProps } from 'reka-ui'

const props = defineProps<ToastProviderProps>()

const forwarded = useForwardProps(props)
</script>

<template>
  <ToastProvider v-bind="forwarded">
    <slot />
  </ToastProvider>
</template>
