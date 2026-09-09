<script setup lang="ts">
// The manifest's variables as a settings sheet: one section per group (General first, Secrets
// last), each a real heading over label-left / control-right rows. Values are strings throughout —
// that is what lands in `.env`. Secrets travel separately and are never echoed.
import VariableField from './VariableField.vue'
import { groupVariables } from '@/lib/deploy/templates'
import type { TemplateManifest } from '@/lib/core/api'

const props = defineProps<{
  manifest: TemplateManifest
  values: Record<string, string>
  secrets: Record<string, string>
  errors: Record<string, string>
  storedSecrets?: string[]
}>()
const emit = defineEmits<{ 'update:values': [Record<string, string>]; 'update:secrets': [Record<string, string>] }>()

function setValue(name: string, v: string): void {
  emit('update:values', { ...props.values, [name]: v })
}
function setSecret(name: string, v: string): void {
  emit('update:secrets', { ...props.secrets, [name]: v })
}
</script>

<template>
  <div class="flex flex-col" data-testid="variables-form">
    <section v-for="(g, i) in groupVariables(manifest)" :key="g.group" :data-group="g.group" :class="['py-4', i > 0 && 'border-t border-border']">
      <div class="mb-1 flex items-baseline gap-2.5">
        <h4 class="text-[13px] font-semibold text-foreground">{{ g.group }}</h4>
        <span class="text-xs text-muted-foreground">{{ g.variables.length }} {{ g.variables.length === 1 ? 'value' : 'values' }}</span>
      </div>
      <VariableField
        v-for="v in g.variables"
        :key="v.name"
        :variable="v"
        :model-value="v.type === 'secret' ? (secrets[v.name] ?? '') : (values[v.name] ?? '')"
        :error="errors[v.name] ?? null"
        :stored="storedSecrets?.includes(v.name)"
        @update:model-value="v.type === 'secret' ? setSecret(v.name, $event) : setValue(v.name, $event)"
      />
    </section>
    <p v-if="!manifest.variables.length" class="text-sm text-muted-foreground">This template has no variables — nothing to fill in.</p>
  </div>
</template>
