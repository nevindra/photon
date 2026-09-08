<script setup lang="ts">
// One typed variable as one row of the settings sheet: the label and its explanation on the
// left, the control on the right. That is what lets a long hint (the uid-1000 warning on the
// PostgreSQL image, say) sit next to its field without pushing the rows below it out of line.
// The control is the right one per type: text, number, switch, choice, port, path, secret
// (write-only password field that says so, with a show/hide toggle so what you typed can be
// checked before it is sealed), generated (read-only: Photon mints it), preset (read-only with
// a lock). The manifest's label and description are the form's copy.
import { ref } from 'vue'
import { Lock, Sparkles, KeyRound, Eye, EyeOff } from 'lucide-vue-next'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { NumberField } from '@/components/ui/number-field'
import { Switch } from '@/components/ui/switch'
import { SelectMenu } from '@/components/ui/select-menu'
import type { TemplateVariable } from '@/lib/core/api'

const props = defineProps<{
  variable: TemplateVariable
  modelValue: string
  error?: string | null
  /** For secrets: the store already holds a value (upgrade of an existing stack). */
  stored?: boolean
}>()
const emit = defineEmits<{ 'update:modelValue': [string] }>()
const id = `var-${props.variable.name}`
const optional = !props.variable.required && props.variable.type !== 'generated' && !props.variable.preset
// Short, identifier-shaped values get a short field so the row does not read as a paragraph box.
const narrow = props.variable.type === 'port' || props.variable.type === 'number' || props.variable.type === 'select'
// Secrets are masked while typed; the toggle reveals only the value in this field, never a
// stored one (the server does not hand those back).
const reveal = ref(false)
</script>

<template>
  <div class="grid gap-1.5 py-2.5 sm:grid-cols-[minmax(0,260px)_minmax(0,1fr)] sm:gap-x-7" :data-variable="variable.name">
    <div class="flex flex-col gap-0.5 sm:pt-2">
      <Label :for="id" class="text-[13.5px] leading-snug">
        {{ variable.label }}
        <span v-if="optional" class="font-normal text-muted-foreground">· optional</span>
      </Label>
      <p v-if="variable.description" class="text-xs leading-relaxed text-muted-foreground">{{ variable.description }}</p>
    </div>
    <div class="flex min-w-0 flex-col gap-1.5">
      <div v-if="variable.type === 'generated'" class="flex min-h-9 max-w-md items-center gap-2 rounded-md border border-dashed border-border bg-muted/40 px-3 py-1.5 text-xs text-muted-foreground" data-kind="generated">
        <Sparkles class="size-3.5 shrink-0 text-brand" /> Photon mints a random value at first deploy and keeps it as a secret. Never shown.
      </div>
      <div v-else-if="variable.preset" class="flex h-9 max-w-md items-center gap-2 rounded-md border border-border bg-muted/40 px-3 font-mono text-sm text-foreground" data-kind="preset">
        <Lock class="size-3.5 shrink-0 text-muted-foreground" /> <span class="truncate">{{ variable.default }}</span>
        <span class="ml-auto shrink-0 font-sans text-[11px] text-muted-foreground">fixed by the template</span>
      </div>
      <div v-else-if="variable.type === 'secret'" class="relative max-w-md" data-kind="secret">
        <KeyRound class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input :id="id" :model-value="modelValue" :type="reveal ? 'text' : 'password'" autocomplete="new-password" class="pl-8 pr-10 font-mono" :placeholder="stored ? 'Stored — leave blank to keep the current value' : 'Entered once, stored encrypted'" @update:model-value="emit('update:modelValue', String($event))" />
        <button
          type="button"
          class="absolute right-1 top-1/2 flex size-7 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
          :aria-label="reveal ? 'Hide password' : 'Show password'"
          :aria-pressed="reveal"
          tabindex="-1"
          data-testid="toggle-secret"
          @click="reveal = !reveal"
        >
          <EyeOff v-if="reveal" class="size-4" />
          <Eye v-else class="size-4" />
        </button>
      </div>
      <div v-else-if="variable.type === 'bool'" class="flex h-9 items-center gap-2" data-kind="bool">
        <Switch :id="id" :model-value="modelValue === 'true'" @update:model-value="emit('update:modelValue', $event ? 'true' : 'false')" />
        <span class="text-sm text-foreground">{{ modelValue === 'true' ? 'Yes' : 'No' }}</span>
      </div>
      <div v-else-if="variable.type === 'select'" class="max-w-[12.5rem]">
        <SelectMenu :id="id" :model-value="modelValue" :options="(variable.options ?? []).map((o) => ({ value: o, label: o }))" content-class="w-56" size="md" :aria-label="variable.label" @update:model-value="emit('update:modelValue', String($event))" />
      </div>
      <div v-else-if="variable.type === 'number'" class="max-w-[12.5rem]">
        <NumberField :id="id" :model-value="Number(modelValue) || 0" @update:model-value="emit('update:modelValue', String($event))" />
      </div>
      <!-- A port is an identifier, not a quantity: no thousands separator, no steppers. -->
      <Input v-else-if="variable.type === 'port'" :id="id" :model-value="modelValue" inputmode="numeric" class="max-w-[12.5rem] font-mono" data-kind="port" @update:model-value="emit('update:modelValue', String($event).replace(/[^0-9]/g, ''))" />
      <Input v-else :id="id" :model-value="modelValue" :class="[variable.type === 'path' && 'font-mono', narrow ? 'max-w-[12.5rem]' : 'max-w-md']" :placeholder="variable.type === 'path' ? '/opt/…' : undefined" @update:model-value="emit('update:modelValue', String($event))" />
      <p v-if="error" class="font-mono text-xs text-sev-error">{{ error }}</p>
    </div>
  </div>
</template>
