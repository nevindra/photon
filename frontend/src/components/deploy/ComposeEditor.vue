<script setup lang="ts">
// The stack's files as editable tabs (compose.yaml, .env, sidecars) with a checks panel that
// updates as you type: the services found, variables the compose references but nothing
// provides, and the dangerous constructs with their acknowledgement boxes. The lint runs locally
// (`lib/deploy/compose.ts`) for instant feedback; the server validates again on save.
import { computed, ref, watch } from 'vue'
import { Plus, X, FileCode, Check, TriangleAlert } from 'lucide-vue-next'
import { Textarea } from '@/components/ui/textarea'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import ComposeWarnings from './ComposeWarnings.vue'
import { lintCompose, composeServices, parseEnv, missingVariables, isComposeFile, isSafePath, FILE_MAX_BYTES } from '@/lib/deploy/compose'
import { cn } from '@/lib/core/utils'
import type { StackFile } from '@/lib/core/api'

const props = defineProps<{
  files: StackFile[]
  acknowledged: string[]
  /** Secret names the store provides, so `${secret:X}` references are not "missing". */
  secrets?: string[]
  readonly?: boolean
  /** Server-side error (from validate/save) to show under the checks. */
  serverError?: string | null
}>()
const emit = defineEmits<{ 'update:files': [StackFile[]]; 'update:acknowledged': [string[]] }>()

const active = ref(props.files[0]?.path ?? 'compose.yaml')
watch(() => props.files.map((f) => f.path).join('|'), () => { if (!props.files.some((f) => f.path === active.value)) active.value = props.files[0]?.path ?? '' })

const current = computed(() => props.files.find((f) => f.path === active.value) ?? null)
function setContent(content: string): void {
  emit('update:files', props.files.map((f) => (f.path === active.value ? { ...f, content } : f)))
}

const adding = ref(false)
const newPath = ref('')
const addError = ref('')
function addFile(): void {
  const path = newPath.value.trim()
  if (!isSafePath(path)) {
    addError.value = 'Relative path inside the stack folder, no “..”.'
    return
  }
  if (props.files.some((f) => f.path === path)) {
    addError.value = 'That file already exists.'
    return
  }
  emit('update:files', [...props.files, { path, content: '' }])
  active.value = path
  adding.value = false
  newPath.value = ''
  addError.value = ''
}
function removeFile(path: string): void {
  if (isComposeFile(path)) return
  emit('update:files', props.files.filter((f) => f.path !== path))
}

const compose = computed(() => props.files.find((f) => isComposeFile(f.path))?.content ?? '')
const services = computed(() => composeServices(compose.value))
const env = computed(() => parseEnv(props.files.find((f) => f.path === '.env')?.content ?? ''))
const missing = computed(() => missingVariables(compose.value, env.value, props.secrets ?? []))
const warnings = computed(() => lintCompose(compose.value))
const unacked = computed(() => warnings.value.filter((w) => !props.acknowledged.includes(w.id)))
const tooBig = computed(() => props.files.filter((f) => new TextEncoder().encode(f.content).length > FILE_MAX_BYTES).map((f) => f.path))
const ready = computed(() => services.value.length > 0 && !unacked.value.length && !tooBig.value.length)

defineExpose({ ready, warnings, services, missing })
</script>

<template>
  <!-- The editor is the whole width; the checks live in its status bar, beside the file name, so
       they update where the eye already is and no side column sits empty next to the text. The
       acknowledgement list for risky constructs appears under the editor only when there are any. -->
  <div class="flex min-w-0 flex-col gap-3" data-testid="compose-editor">
    <div class="flex min-w-0 flex-col overflow-hidden rounded-xl border border-border bg-card shadow-1">
      <div class="flex items-center gap-0.5 border-b border-border bg-muted/40 px-2 pt-2">
        <button
          v-for="f in files"
          :key="f.path"
          type="button"
          :data-file="f.path"
          :class="cn('group flex items-center gap-1.5 rounded-t-md border border-b-0 px-3 py-1.5 font-mono text-xs transition-colors', f.path === active ? 'border-border bg-card text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground')"
          @click="active = f.path"
        >
          <FileCode class="size-3.5" /> {{ f.path }}
          <X v-if="!readonly && !isComposeFile(f.path)" class="size-3 opacity-0 transition-opacity hover:text-sev-error group-hover:opacity-100" @click.stop="removeFile(f.path)" />
        </button>
        <template v-if="!readonly">
          <form v-if="adding" class="ml-1 flex items-center gap-1" @submit.prevent="addFile">
            <Input v-model="newPath" placeholder="conf/nginx.conf" class="h-7 w-44 font-mono text-xs" data-testid="new-file-path" autofocus />
            <Button type="submit" size="sm" variant="outline" class="h-7 px-2 text-xs">Add</Button>
            <Button type="button" size="sm" variant="ghost" class="h-7 px-2 text-xs" @click="adding = false; addError = ''">Cancel</Button>
            <span v-if="addError" class="text-[11px] text-sev-error">{{ addError }}</span>
          </form>
          <Button v-else type="button" size="sm" variant="ghost" class="ml-1 h-7 px-2 text-xs" data-testid="add-file" @click="adding = true"><Plus class="size-3.5" /> File</Button>
        </template>
      </div>
      <Textarea
        v-if="current"
        :model-value="current.content"
        mono
        :disabled="readonly"
        spellcheck="false"
        class="min-h-[18rem] resize-y rounded-none border-0 focus-visible:ring-0"
        data-testid="file-content"
        :data-file="current.path"
        @update:model-value="setContent($event)"
      />
      <div class="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border bg-muted/40 px-3 py-1.5 text-[11px] text-muted-foreground">
        <ul class="flex flex-wrap items-center gap-x-4 gap-y-1" data-testid="compose-checks">
          <li class="flex items-center gap-1.5" data-check="services" :title="services.length ? services.join(', ') : undefined">
            <component :is="services.length ? Check : TriangleAlert" class="size-3.5 shrink-0" :class="services.length ? 'text-success' : 'text-sev-error'" />
            <span class="text-foreground">{{ services.length ? `${services.length} ${services.length === 1 ? 'service' : 'services'}: ` : 'No services under services:' }}<span class="font-mono text-muted-foreground">{{ services.join(', ') }}</span></span>
          </li>
          <li class="flex items-center gap-1.5" data-check="variables" :title="missing.length ? 'Add them to .env or the secret store.' : undefined">
            <component :is="missing.length ? TriangleAlert : Check" class="size-3.5 shrink-0" :class="missing.length ? 'text-sev-warn' : 'text-success'" />
            <span class="text-foreground">{{ missing.length ? 'Unset variables: ' : 'Every variable resolves.' }}<span class="font-mono text-muted-foreground">{{ missing.join(', ') }}</span></span>
          </li>
          <li class="flex items-center gap-1.5" data-check="warnings">
            <component :is="unacked.length ? TriangleAlert : Check" class="size-3.5 shrink-0" :class="unacked.length ? 'text-sev-warn' : 'text-success'" />
            <span class="text-foreground">{{ warnings.length ? `${warnings.length} risky ${warnings.length === 1 ? 'construct' : 'constructs'}, ${unacked.length} unacknowledged` : 'No risky constructs.' }}</span>
          </li>
          <li v-if="tooBig.length" class="flex items-center gap-1.5 text-sev-error" data-check="size"><TriangleAlert class="size-3.5 shrink-0" /> Over 10 MiB: {{ tooBig.join(', ') }}</li>
        </ul>
        <span class="ml-auto flex flex-wrap items-center gap-x-3">
          <span><span class="font-mono">{{ current?.path }}</span> · {{ (current?.content.length ?? 0).toLocaleString() }} chars</span>
          <span v-pre title="Secrets never go in these files: reference them as ${secret:NAME} and the agent resolves them at apply time. Paths stay inside the stack folder; 10 MiB per file.">secrets as <code class="font-mono">${secret:NAME}</code></span>
        </span>
      </div>
    </div>
    <ComposeWarnings v-if="warnings.length" :warnings="warnings" :acknowledged="acknowledged" :readonly="readonly" @update:acknowledged="emit('update:acknowledged', $event)" />
    <p v-if="serverError" class="flex items-center gap-1.5 font-mono text-xs text-sev-error" data-check="server"><TriangleAlert class="size-3.5 shrink-0" /> {{ serverError }}</p>
  </div>
</template>
