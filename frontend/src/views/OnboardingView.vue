<script setup>
import { ref, computed } from 'vue'
import { useRouter } from 'vue-router'
import AuthScaffold from '@/components/common/AuthScaffold.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { setup } from '@/lib/core/auth'

const router = useRouter()

const username = ref('')
const password = ref('')
const confirm = ref('')
const error = ref('')
const busy = ref(false)

const mismatch = computed(() => confirm.value.length > 0 && password.value !== confirm.value)

async function submit() {
  if (busy.value) return
  error.value = ''
  if (password.value.length < 8) {
    error.value = 'Password must be at least 8 characters.'
    return
  }
  if (password.value !== confirm.value) {
    error.value = 'Passwords do not match.'
    return
  }
  busy.value = true
  try {
    const res = await setup(username.value, password.value)
    if (res.ok) router.push('/logs')
    else error.value = res.error || 'Could not create the account.'
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <AuthScaffold subtitle="Create the first account to get started.">
    <form class="mt-8 flex flex-col gap-4" @submit.prevent="submit">
      <FormField label="Username" for="username">
        <Input id="username" v-model="username" type="text" autocomplete="username" autofocus />
      </FormField>
      <FormField label="Password" for="password">
        <Input id="password" v-model="password" type="password" autocomplete="new-password" />
      </FormField>
      <FormField label="Confirm password" for="confirm">
        <Input id="confirm" v-model="confirm" type="password" autocomplete="new-password" />
      </FormField>

      <Alert v-if="mismatch || error" variant="error">
        <AlertDescription>
          {{ mismatch ? 'Passwords do not match.' : error }}
        </AlertDescription>
      </Alert>

      <Button variant="brand" type="submit" class="mt-1 w-full" :disabled="busy">
        {{ busy ? 'Creating…' : 'Create account' }}
      </Button>
    </form>
  </AuthScaffold>
</template>
