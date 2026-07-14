<script setup>
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AuthScaffold from '@/components/common/AuthScaffold.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { login } from '@/lib/core/auth'

const route = useRoute()
const router = useRouter()

const username = ref('')
const password = ref('')
const error = ref('')
const busy = ref(false)

async function submit() {
  if (busy.value) return
  error.value = ''
  busy.value = true
  try {
    const res = await login(username.value, password.value)
    if (res.ok) {
      const redirect = typeof route.query.redirect === 'string' ? route.query.redirect : '/logs'
      router.push(redirect)
    } else {
      error.value = "That username and password don't match."
    }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <AuthScaffold subtitle="Fast, lightweight log search.">
    <form class="mt-8 flex flex-col gap-4" @submit.prevent="submit">
      <FormField label="Username" for="username">
        <Input id="username" v-model="username" type="text" autocomplete="username" autofocus />
      </FormField>
      <FormField label="Password" for="password">
        <Input
          id="password"
          v-model="password"
          type="password"
          autocomplete="current-password"
        />
      </FormField>

      <Alert v-if="error" variant="error">
        <AlertDescription>{{ error }}</AlertDescription>
      </Alert>

      <Button variant="brand" type="submit" class="mt-1 w-full" :disabled="busy">
        {{ busy ? 'Signing in…' : 'Sign in' }}
      </Button>
    </form>
  </AuthScaffold>
</template>
