<script setup lang="ts">
// Removing a person is one confirmation, not a typed name: it is reversible in the sense that
// matters (add them again) and touches nothing they deployed. What it does do is stated once,
// plainly — the session ends and the audit log keeps their name.
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import UserAvatar from './UserAvatar.vue'
import RoleBadge from './RoleBadge.vue'
import type { UserInfo } from '@/lib/core/api'
import { roleOf } from '@/lib/core/users'

defineProps<{ open: boolean; user: UserInfo | null; rolesSupported: boolean; pending?: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; confirm: [] }>()
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent v-if="user" class="max-w-md" data-testid="remove-user-dialog">
      <DialogHeader>
        <DialogTitle>Remove {{ user.username }}?</DialogTitle>
        <DialogDescription>
          They are signed out now and can no longer sign in. Nothing they deployed changes, and the audit log keeps their name on what they did.
        </DialogDescription>
      </DialogHeader>
      <div class="flex items-center gap-3 rounded-lg border border-border bg-muted/40 px-3 py-2.5">
        <UserAvatar :username="user.username" size="md" />
        <div class="flex min-w-0 flex-col gap-1">
          <span class="truncate text-sm font-medium text-foreground">{{ user.username }}</span>
          <RoleBadge v-if="rolesSupported" :role="roleOf(user)" class="w-fit" />
        </div>
      </div>
      <DialogFooter>
        <Button variant="outline" :disabled="pending" @click="emit('update:open', false)">Cancel</Button>
        <Button variant="destructive" :disabled="pending" data-testid="remove-user-confirm" @click="emit('confirm')">
          {{ pending ? 'Removing…' : 'Remove user' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
