<script setup lang="ts">
// The one place a person is added: a modal off the "Add a user" button, so the table keeps the
// whole width and the form appears only when someone means to fill it. Validation is the server's
// rules restated before the round trip, shown under the field it belongs to and only once the
// field has been touched — a form that shouts "required" at an empty dialog has not been used
// yet. The password is typed twice (`PasswordFields`): Photon never shows it again. The role is
// three radio cards, each saying in one line what it lets the person do — the choice that matters
// most should not live behind a hover. The form resets every time the dialog opens, so a name
// abandoned last time is not sitting there for the next person.
import { ref, computed, watch } from 'vue'
import { Check, UserRoundPlus } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import PasswordFields from './PasswordFields.vue'
import { cn } from '@/lib/core/utils'
import { ROLES, ROLE_META, type Role } from '@/lib/deploy/roles'
import { validateCredentials } from '@/lib/core/users'

const props = defineProps<{
  open: boolean
  /** A server without roles has one implicit role; the picker would toast success over a request it drops. */
  rolesSupported: boolean
  busy: boolean
  /** Set by the parent when the server rejected the last submit (409 duplicate, 400 rule). */
  serverError: string | null
}>()
const emit = defineEmits<{
  'update:open': [open: boolean]
  submit: [input: { username: string; password: string; role: Role }]
  'clear-error': []
}>()

const username = ref('')
const password = ref('')
const confirm = ref('')
const role = ref<Role>('viewer')
const touched = ref({ username: false, password: false, confirm: false })
const submitted = ref(false)
const fields = ref<InstanceType<typeof PasswordFields> | null>(null)

const errors = computed(() => validateCredentials(username.value, password.value, confirm.value))
const valid = computed(() => Object.keys(errors.value).length === 0)
const show = (f: 'username' | 'password' | 'confirm') => (touched.value[f] || submitted.value ? errors.value[f] : undefined)
const passwordErrors = computed(() => ({ password: show('password'), confirm: show('confirm') }))

// The server's answer belongs under the field it is about: a duplicate name under Username,
// anything else at the bottom next to the button.
const usernameServerError = computed(() => (props.serverError && /exist|username/i.test(props.serverError) ? props.serverError : undefined))
const footerError = computed(() => (props.serverError && !usernameServerError.value ? props.serverError : null))
watch(username, () => {
  // Typing again after a rejection means "I heard you"; the message would now be about a different name.
  if (usernameServerError.value) emit('clear-error')
})

function submit(): void {
  submitted.value = true
  if (!valid.value || props.busy) return
  emit('submit', { username: username.value.trim(), password: password.value, role: role.value })
}

function reset(): void {
  username.value = ''
  password.value = ''
  confirm.value = ''
  role.value = 'viewer'
  touched.value = { username: false, password: false, confirm: false }
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
  // Nothing closes under a request in flight — the answer would land on a dialog that is gone.
  if (!o && props.busy) return
  emit('update:open', o)
}
</script>

<template>
  <Dialog :open="open" @update:open="onOpen">
    <DialogContent class="max-w-md" data-testid="add-user-dialog">
      <DialogHeader>
        <div class="flex items-center gap-2">
          <span class="flex size-8 items-center justify-center rounded-lg bg-brand-soft text-brand">
            <UserRoundPlus class="size-4" />
          </span>
          <DialogTitle>Add a user</DialogTitle>
        </div>
        <DialogDescription>They sign in with these. Share the password over a channel you trust; Photon never shows it again.</DialogDescription>
      </DialogHeader>

      <form id="add-user-form" class="flex flex-col gap-4" novalidate @submit.prevent="submit">
        <FormField label="Username" for="new-username" :error="show('username') ?? usernameServerError">
          <Input
            id="new-username"
            v-model="username"
            type="text"
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
            placeholder="e.g. rina"
            :disabled="busy"
            data-testid="new-username"
            @blur="touched.username = true"
          />
        </FormField>

        <PasswordFields
          ref="fields"
          v-model:password="password"
          v-model:confirm="confirm"
          :errors="passwordErrors"
          :disabled="busy"
          id-prefix="new"
          @blur="(f) => (touched[f] = true)"
        />

        <fieldset v-if="rolesSupported" class="flex flex-col gap-1.5" :disabled="busy">
          <legend class="mb-1.5 text-sm font-medium leading-none text-foreground">Role</legend>
          <label
            v-for="r in ROLES"
            :key="r"
            :class="
              cn(
                'flex cursor-pointer items-start gap-3 rounded-lg border px-3 py-2.5 transition-[border-color,background-color,box-shadow] duration-150 ease-out has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring/30',
                role === r ? 'border-brand/40 bg-brand-soft/60 shadow-1' : 'border-border hover:border-foreground/15 hover:bg-muted/50',
                busy && 'cursor-not-allowed opacity-60',
              )
            "
            :data-testid="`role-option-${r}`"
            :data-checked="role === r"
          >
            <input v-model="role" type="radio" name="new-role" :value="r" class="sr-only" />
            <span
              class="mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border transition-colors"
              :class="role === r ? 'border-brand bg-brand text-brand-foreground' : 'border-foreground/25 bg-surface-1'"
              aria-hidden="true"
            >
              <Check v-if="role === r" class="size-2.5" stroke-width="3" />
            </span>
            <span class="flex min-w-0 flex-col gap-0.5">
              <span class="text-sm font-medium leading-none text-foreground">{{ ROLE_META[r].label }}</span>
              <span class="text-xs leading-snug text-muted-foreground">{{ ROLE_META[r].summary }}</span>
            </span>
          </label>
        </fieldset>

        <p v-if="footerError" class="font-mono text-xs text-sev-error" data-testid="add-user-error">{{ footerError }}</p>
      </form>

      <DialogFooter>
        <Button type="button" variant="outline" :disabled="busy" data-testid="add-user-cancel" @click="onOpen(false)">Cancel</Button>
        <Button type="submit" form="add-user-form" :disabled="busy" data-testid="add-user-submit">
          {{ busy ? 'Adding…' : 'Add user' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
