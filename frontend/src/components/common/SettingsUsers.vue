<script setup>
import { ref, computed } from 'vue'
import { Trash2 } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Spinner } from '@/components/ui/spinner'
import { FormField } from '@/components/ui/form-field'
import { username as currentUsername } from '@/lib/core/auth'
import { useUsers, useCreateUser, useDeleteUser } from '@/lib/core/usersQueries'

const { data, isLoading } = useUsers()
const createUser = useCreateUser()
const deleteUser = useDeleteUser()

const users = computed(() => data.value?.users ?? [])

const newUsername = ref('')
const newPassword = ref('')
const error = ref('')
const busy = ref(false)

async function add() {
  if (busy.value) return
  error.value = ''
  if (newUsername.value.trim() === '' || newPassword.value.length < 8) {
    error.value = 'Username required and password must be at least 8 characters.'
    return
  }
  busy.value = true
  try {
    const res = await createUser.mutateAsync({
      username: newUsername.value.trim(),
      password: newPassword.value,
    })
    if (res && res.ok === false) {
      error.value = res.error || 'Could not add user.'
      return
    }
    newUsername.value = ''
    newPassword.value = ''
  } finally {
    busy.value = false
  }
}

async function remove(name) {
  error.value = ''
  const res = await deleteUser.mutateAsync(name)
  if (res && res.ok === false) error.value = res.error || 'Could not remove user.'
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <Spinner v-if="isLoading" size="sm">Loading…</Spinner>
    <ul v-else class="flex flex-col divide-y divide-border rounded-md border border-border">
      <li
        v-for="u in users"
        :key="u.username"
        class="flex items-center justify-between px-3 py-2"
      >
        <span class="text-sm text-card-foreground">
          {{ u.username }}
          <span v-if="u.username === currentUsername" class="ml-1 text-xs text-muted-foreground"
            >(you)</span
          >
        </span>
        <Button
          variant="ghost"
          size="icon"
          class="size-7 text-muted-foreground hover:text-sev-error"
          :disabled="u.username === currentUsername || users.length <= 1"
          aria-label="Remove user"
          @click="remove(u.username)"
        >
          <Trash2 class="size-4" />
        </Button>
      </li>
    </ul>
  </div>

  <form class="mt-2 flex flex-col gap-3 border-t border-border pt-4" @submit.prevent="add">
    <p class="text-sm font-medium text-card-foreground">Add a user</p>
    <FormField label="Username" for="new-username">
      <Input id="new-username" v-model="newUsername" type="text" autocomplete="off" />
    </FormField>
    <FormField label="Password" for="new-password">
      <Input id="new-password" v-model="newPassword" type="password" autocomplete="new-password" />
    </FormField>
    <p v-if="error" class="font-mono text-xs text-sev-error">{{ error }}</p>
    <Button type="submit" :disabled="busy">{{ busy ? 'Adding…' : 'Add user' }}</Button>
  </form>
</template>
