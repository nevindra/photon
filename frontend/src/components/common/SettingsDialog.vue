<script setup>
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import SettingsUsers from './SettingsUsers.vue'

defineProps({ open: { type: Boolean, default: false } })
const emit = defineEmits(['update:open'])
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-w-2xl">
      <DialogHeader>
        <DialogTitle>Settings</DialogTitle>
        <DialogDescription>Manage users.</DialogDescription>
      </DialogHeader>
      <SettingsUsers />
    </DialogContent>
  </Dialog>
</template>
