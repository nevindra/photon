<script setup lang="ts">
// Mount this once at the app root (see App.vue). It owns no state of its own — it just reads
// the shared `useToast()` queue and renders one `<Toast>` per entry. `<ToastRoot>` (used inside
// `Toast.vue`) auto-teleports itself into the `<ToastViewport>` DOM node via reka-ui's internal
// Teleport, so the `<Toast>` loop and the portaled viewport can be declared as siblings here.
import { ToastPortal } from 'reka-ui'
import Toast from './Toast.vue'
import ToastTitle from './ToastTitle.vue'
import ToastDescription from './ToastDescription.vue'
import ToastClose from './ToastClose.vue'
import ToastViewport from './ToastViewport.vue'
import ToastProvider from './ToastProvider.vue'
import { useToast } from './useToast'

const { toasts, dismiss } = useToast()
</script>

<template>
  <ToastProvider>
    <Toast
      v-for="t in toasts"
      :key="t.id"
      :variant="t.variant"
      :open="t.open"
      :duration="t.duration"
      @update:open="(open) => !open && dismiss(t.id)"
    >
      <div class="flex flex-col gap-1">
        <ToastTitle v-if="t.title">{{ t.title }}</ToastTitle>
        <ToastDescription v-if="t.description">{{ t.description }}</ToastDescription>
      </div>
      <ToastClose />
    </Toast>

    <ToastPortal>
      <ToastViewport />
    </ToastPortal>
  </ToastProvider>
</template>
