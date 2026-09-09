<script setup lang="ts">
// Setting a password, in two shapes that share one dialog. Your own: the current password first,
// because the session cookie proves who is sitting here, not that they are the one who set it.
// Someone else's, as an admin: no current password — a reset is what you do when the person has
// forgotten it — and a line saying to hand the new one over yourself, since Photon will not.
// The new password is typed twice (`PasswordFields`). Signed-in sessions survive the change:
// a person who changes their own password is not thrown out of the page they did it on.
import { ref, computed, watch } from 'vue'
import { KeyRound } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import PasswordFields from './PasswordFields.vue'
import UserAvatar from './UserAvatar.vue'
import type { UserInfo } from '@/lib/core/api'
import { validatePasswordChange } from '@/lib/core/users'

const props = defineProps<{
  open: boolean
  user: UserInfo | null
  /** The row is the signed-in user's own — ask for the current password. */
  self: boolean
  pending: boolean
  /** The server's refusal (wrong current password, rule, 403). */
  serverError: string | null
}>()
const emit = defineEmits<{
  'update:open': [open: boolean]
  submit: [input: { password: string; currentPassword?: string }]
  'clear-error': []
}>()

const current = ref('')
const password = ref('')
const confirm = ref('')
const touched = ref({ current: false, password: false, confirm: false })
const submitted = ref(false)
const fields = ref<InstanceType<typeof PasswordFields> | null>(null)

const errors = computed(() =>
  validatePasswordChange({ password: password.value, confirm: confirm.value, current: current.value, requireCurrent: props.self }),
)
const valid = computed(() => Object.keys(errors.value).length === 0)
const show = (f: 'current' | 'password' | 'confirm') => (touched.value[f] || submitted.value ? errors.value[f] : undefined)
const passwordErrors = computed(() => ({ password: show('password'), confirm: show('confirm') }))

// A wrong current password belongs under that field; anything else next to the button.
const currentServerError = computed(() => (props.serverError && /current/i.test(props.serverError) ? props.serverError : undefined))
const footerError = computed(() => (props.serverError && !currentServerError.value ? props.serverError : null))
watch(current, () => {
  if (currentServerError.value) emit('clear-error')
})

function submit(): void {
  submitted.value = true
  if (!valid.value || props.pending) return
  emit('submit', { password: password.value, currentPassword: props.self ? current.value : undefined })
}

function reset(): void {
  current.value = ''
  password.value = ''
  confirm.value = ''
  touched.value = { current: false, password: false, confirm: false }
  submitted.value = false
  fields.value?.reset()
}
watch(
  () => props.open,
  (o) => {
    if (o) reset()
    else emit('clear-error')
  },
)
function onOpen(o: boolean): void {
  if (!o && props.pending) return
  emit('update:open', o)
}
</script>

<template>
  <Dialog :open="open" @update:open="onOpen">
    <DialogContent v-if="user" class="max-w-md" data-testid="change-password-dialog">
      <DialogHeader>
        <div class="flex items-center gap-2">
          <span class="flex size-8 items-center justify-center rounded-lg bg-brand-soft text-brand">
            <KeyRound class="size-4" />
          </span>
          <DialogTitle>{{ self ? 'Change your password' : `Reset password for ${user.username}` }}</DialogTitle>
        </div>
        <DialogDescription>
          <template v-if="self">You stay signed in here. Anywhere else, the next sign-in needs the new one.</template>
          <template v-else>They keep any session that is open now. Hand them the new password over a channel you trust; Photon never shows it again.</template>
        </DialogDescription>
      </DialogHeader>

      <div v-if="!self" class="flex items-center gap-3 rounded-lg border border-border bg-muted/40 px-3 py-2.5">
        <UserAvatar :username="user.username" size="md" />
        <span class="truncate text-sm font-medium text-foreground">{{ user.username }}</span>
      </div>

      <form id="change-password-form" class="flex flex-col gap-4" novalidate @submit.prevent="submit">
        <FormField v-if="self" label="Current password" for="current-password" :error="show('current') ?? currentServerError">
          <Input
            id="current-password"
            v-model="current"
            type="password"
            autocomplete="current-password"
            :disabled="pending"
            data-testid="current-password"
            @blur="touched.current = true"
          />
        </FormField>

        <PasswordFields
          ref="fields"
          v-model:password="password"
          v-model:confirm="confirm"
          :errors="passwordErrors"
          :disabled="pending"
          label="New password"
          id-prefix="change"
          @blur="(f) => (touched[f] = true)"
        />

        <p v-if="footerError" class="font-mono text-xs text-sev-error" data-testid="change-password-error">{{ footerError }}</p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="pending" @click="onOpen(false)">Cancel</Button>
        <Button type="submit" form="change-password-form" :disabled="pending" data-testid="change-password-submit">
          {{ pending ? 'Saving…' : self ? 'Change password' : 'Set password' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
