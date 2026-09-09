<script setup lang="ts">
// A password typed twice. It is set by an admin on behalf of someone else, or by a person who
// will not see it again after the dialog closes, so a slip of a finger is a lockout — the
// confirmation catches that before the round trip. One reveal toggle uncovers both: once you can
// read it, typing it twice is only a formality, but the rule stays so the flow is one flow.
// Each field carries a live check that turns into a tick the moment its rule is met, so nobody
// counts characters or squints at two rows of dots.
import { ref, computed } from 'vue'
import { Eye, EyeOff, Check } from 'lucide-vue-next'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { MIN_PASSWORD_LEN } from '@/lib/core/users'

const props = withDefaults(
  defineProps<{
    /** Shown under each field only once the parent decides it is time (touched or submitted). */
    errors?: { password?: string; confirm?: string }
    disabled?: boolean
    /** "Password" on a new account, "New password" on a change. */
    label?: string
    /** Prefix for the two input ids, so two dialogs never share a label target. */
    idPrefix?: string
  }>(),
  { errors: () => ({}), disabled: false, label: 'Password', idPrefix: 'pw' },
)
const emit = defineEmits<{ blur: [field: 'password' | 'confirm'] }>()

const password = defineModel<string>('password', { default: '' })
const confirm = defineModel<string>('confirm', { default: '' })
const reveal = ref(false)

const passwordOk = computed(() => password.value.length >= MIN_PASSWORD_LEN)
const confirmOk = computed(() => passwordOk.value && confirm.value !== '' && confirm.value === password.value)

/** The parent calls this when it resets the form, so a revealed password is not left readable for the next person. */
function reset(): void {
  reveal.value = false
}
defineExpose({ reset })
</script>

<template>
  <FormField :label="label" :for="`${idPrefix}-password`" :error="errors.password">
    <div class="relative">
      <Input
        :id="`${idPrefix}-password`"
        v-model="password"
        :type="reveal ? 'text' : 'password'"
        autocomplete="new-password"
        class="pr-10"
        :placeholder="`At least ${MIN_PASSWORD_LEN} characters`"
        :disabled="disabled"
        data-testid="new-password"
        @blur="emit('blur', 'password')"
      />
      <button
        type="button"
        class="absolute right-1 top-1/2 flex size-7 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
        :aria-label="reveal ? 'Hide password' : 'Show password'"
        :aria-pressed="reveal"
        tabindex="-1"
        data-testid="toggle-password"
        @click="reveal = !reveal"
      >
        <EyeOff v-if="reveal" class="size-4" />
        <Eye v-else class="size-4" />
      </button>
    </div>
    <!-- Hidden while the error row says the same thing. -->
    <p v-if="!errors.password" class="flex items-center gap-1 text-xs" :class="passwordOk ? 'text-success' : 'text-muted-foreground'" data-testid="password-hint">
      <Check v-if="passwordOk" class="size-3.5" />
      {{ passwordOk ? 'Long enough.' : `At least ${MIN_PASSWORD_LEN} characters.` }}
    </p>
  </FormField>

  <FormField label="Confirm password" :for="`${idPrefix}-confirm`" :error="errors.confirm">
    <Input
      :id="`${idPrefix}-confirm`"
      v-model="confirm"
      :type="reveal ? 'text' : 'password'"
      autocomplete="new-password"
      placeholder="Type it again"
      :disabled="disabled"
      data-testid="confirm-password"
      @blur="emit('blur', 'confirm')"
    />
    <p v-if="!errors.confirm && confirmOk" class="flex items-center gap-1 text-xs text-success" data-testid="confirm-hint">
      <Check class="size-3.5" />
      Matches.
    </p>
  </FormField>
</template>
