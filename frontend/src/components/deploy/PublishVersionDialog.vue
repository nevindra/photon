<script setup lang="ts">
// Publish a template version: structured manifest fields (no YAML to hand-write — the preview
// shows the `photon.template.yaml` that results), the compose file, and a typed variables table.
// Every publish is a NEW immutable version; editing an existing one is not a thing.
import { computed, ref, watch } from 'vue'
import { Plus, Trash2, Loader2, Upload } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { FormField } from '@/components/ui/form-field'
import { SelectMenu } from '@/components/ui/select-menu'
import { Checkbox } from '@/components/ui/checkbox'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import GatedButton from './GatedButton.vue'
import { useTemplateVersion, usePublishTemplateVersion } from '@/lib/deploy/deployQueries'
import { VARIABLE_TYPES, VARIABLE_TYPE_META, manifestToYaml, nextVersion } from '@/lib/deploy/templates'
import { composeServices, composeVariableRefs, lintCompose } from '@/lib/deploy/compose'
import type { Template, TemplateManifest, TemplateVariable, VariableType } from '@/lib/core/api'

const props = defineProps<{ open: boolean; template?: Template | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

// Seed from the latest version when publishing onto an existing template.
const latestId = computed(() => props.template?.versions[0]?.id ?? null)
const latestQ = useTemplateVersion(latestId)

const m = ref<TemplateManifest>(blank())
const compose = ref('')
const notes = ref('')
const error = ref('')
function blank(): TemplateManifest {
  return { id: '', version: '1.0.0', name: '', description: '', icon: '', categories: [], variables: [], requires: { arch: ['x86_64'], min_docker: '24.0', min_ram_bytes: null, min_disk_bytes: null }, ports: [], volumes: [], health: null, observability: { otlp_endpoint: true, ingest_token: true, service_name: null, environment: true } }
}
watch([() => props.open, () => latestQ.data.value], ([o, latest]) => {
  if (!o) return
  if (props.template && latest) {
    m.value = { ...JSON.parse(JSON.stringify(latest.manifest)), version: nextVersion(latest.version) }
    compose.value = latest.compose_text
  } else if (!props.template) {
    m.value = blank()
    compose.value = ''
  }
  notes.value = ''
  error.value = ''
}, { immediate: true })

const categories = computed({ get: () => m.value.categories.join(', '), set: (v: string) => (m.value.categories = v.split(',').map((s) => s.trim()).filter(Boolean)) })
const arch = computed({ get: () => (m.value.requires.arch ?? []).join(', '), set: (v: string) => (m.value.requires.arch = v.split(',').map((s) => s.trim()).filter(Boolean)) })
const ramGb = computed({ get: () => (m.value.requires.min_ram_bytes ? m.value.requires.min_ram_bytes / 1024 ** 3 : ''), set: (v: string | number) => (m.value.requires.min_ram_bytes = v === '' ? null : Number(v) * 1024 ** 3) })
const diskGb = computed({ get: () => (m.value.requires.min_disk_bytes ? m.value.requires.min_disk_bytes / 1024 ** 3 : ''), set: (v: string | number) => (m.value.requires.min_disk_bytes = v === '' ? null : Number(v) * 1024 ** 3) })

function addVariable(name = ''): void {
  m.value.variables.push({ name, label: name, type: 'string', required: true, default: '', preset: false, options: [], pattern: null, group: null })
}
function removeVariable(i: number): void {
  m.value.variables.splice(i, 1)
}
// Variables the compose references that the manifest does not declare — one click adds them.
const undeclared = computed(() => composeVariableRefs(compose.value).filter((r) => !m.value.variables.some((v) => v.name === r)))
const services = computed(() => composeServices(compose.value))
const warnings = computed(() => lintCompose(compose.value))
const yaml = computed(() => manifestToYaml(m.value))
const TYPE_OPTS = VARIABLE_TYPES.map((t) => ({ value: t, label: VARIABLE_TYPE_META[t].label }))

const publish = usePublishTemplateVersion()
async function submit(): Promise<void> {
  error.value = ''
  const res = await publish.mutateAsync({ template_id: props.template?.id ?? null, manifest: m.value, compose_text: compose.value, notes: notes.value || null })
  if (res.ok) emit('update:open', false)
  else error.value = res.error ?? 'Could not publish.'
}
const canSubmit = computed(() => !!m.value.id && !!m.value.name && !!m.value.version && services.value.length > 0)
function setType(v: TemplateVariable, t: VariableType): void {
  v.type = t
  if (t === 'select' && !v.options?.length) v.options = ['']
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-h-[92vh] max-w-5xl overflow-y-auto [&>*]:min-w-0" data-testid="publish-dialog">
      <DialogHeader>
        <DialogTitle>{{ template ? `Publish ${template.name} ${m.version}` : 'New template' }}</DialogTitle>
        <DialogDescription>A compose file plus a manifest. Versions are immutable: "which version is customer X on" stays answerable because nothing here is ever edited in place.</DialogDescription>
      </DialogHeader>

      <Tabs default-value="about" class="flex flex-col gap-4">
        <TabsList class="w-fit">
          <TabsTrigger value="about">About</TabsTrigger>
          <TabsTrigger value="compose">Compose <span v-if="services.length" class="ml-1 text-muted-foreground">{{ services.length }}</span></TabsTrigger>
          <TabsTrigger value="variables">Variables <span class="ml-1 text-muted-foreground">{{ m.variables.length }}</span></TabsTrigger>
          <TabsTrigger value="requires">Requires & health</TabsTrigger>
          <TabsTrigger value="manifest">Manifest</TabsTrigger>
        </TabsList>

        <TabsContent value="about" class="grid gap-3 sm:grid-cols-2">
          <FormField label="Template id" for="tpl-id" hint="Stable; becomes the default stack name."><Input id="tpl-id" v-model="m.id" :disabled="!!template" class="font-mono" placeholder="nodeflux-vas" /></FormField>
          <FormField label="Version" for="tpl-version"><Input id="tpl-version" v-model="m.version" class="font-mono" placeholder="2.4.2" /></FormField>
          <FormField label="Name" for="tpl-name"><Input id="tpl-name" v-model="m.name" placeholder="Nodeflux VAS" /></FormField>
          <FormField label="Icon" for="tpl-icon" hint="An emoji, or an image URL."><Input id="tpl-icon" v-model="m.icon" placeholder="🎥" /></FormField>
          <FormField label="Description" for="tpl-desc" class="sm:col-span-2"><Textarea id="tpl-desc" v-model="m.description" class="min-h-[3.5rem]" /></FormField>
          <FormField label="Categories" for="tpl-cats" hint="Comma-separated."><Input id="tpl-cats" v-model="categories" placeholder="nodeflux, edge" /></FormField>
          <FormField label="Release notes" for="tpl-notes" optional><Input id="tpl-notes" v-model="notes" placeholder="What changed in this version" /></FormField>
        </TabsContent>

        <TabsContent value="compose" class="flex flex-col gap-2">
          <Textarea v-model="compose" mono class="min-h-[22rem]" spellcheck="false" placeholder="services:&#10;  app:&#10;    image: …" data-testid="tpl-compose" />
          <div class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
            <span>{{ services.length ? `services: ${services.join(', ')}` : 'no services yet' }}</span>
            <span v-if="warnings.length" class="text-sev-warn">{{ warnings.length }} risky {{ warnings.length === 1 ? 'construct' : 'constructs' }} — operators will have to acknowledge {{ warnings.length === 1 ? 'it' : 'them' }} at deploy</span>
            <span v-if="undeclared.length">undeclared variables: <span class="font-mono text-foreground">{{ undeclared.join(', ') }}</span> <button type="button" class="text-brand hover:underline" @click="undeclared.forEach((u) => addVariable(u))">declare them</button></span>
          </div>
          <p class="text-[11px] text-muted-foreground">Reference variables as <code v-pre class="font-mono">${NAME}</code>; secrets and generated values as <code v-pre class="font-mono">${secret:NAME}</code>. The Photon block injects <code class="font-mono">OTEL_EXPORTER_OTLP_ENDPOINT</code> and the ingest token as variables.</p>
        </TabsContent>

        <TabsContent value="variables" class="flex flex-col gap-2">
          <div v-for="(v, i) in m.variables" :key="i" class="grid gap-2 rounded-xl border border-border bg-card p-3 sm:grid-cols-[1fr_1fr_8rem_1fr]" :data-var="v.name">
            <FormField label="Name" :for="`v-${i}-name`"><Input :id="`v-${i}-name`" v-model="v.name" class="font-mono" placeholder="API_PORT" /></FormField>
            <FormField label="Label" :for="`v-${i}-label`"><Input :id="`v-${i}-label`" v-model="v.label" placeholder="API port" /></FormField>
            <FormField label="Type" :for="`v-${i}-type`"><SelectMenu :id="`v-${i}-type`" :model-value="v.type" :options="TYPE_OPTS" content-class="w-44" size="md" @update:model-value="setType(v, $event as VariableType)" /></FormField>
            <FormField v-if="v.type !== 'generated' && v.type !== 'secret'" label="Default" :for="`v-${i}-default`"><Input :id="`v-${i}-default`" :model-value="v.default ?? ''" class="font-mono" @update:model-value="v.default = String($event)" /></FormField>
            <div v-else class="text-xs text-muted-foreground sm:pt-6">{{ VARIABLE_TYPE_META[v.type].summary }}</div>
            <FormField label="Description" :for="`v-${i}-desc`" class="sm:col-span-2"><Input :id="`v-${i}-desc`" :model-value="v.description ?? ''" @update:model-value="v.description = String($event)" /></FormField>
            <FormField v-if="v.type === 'select'" label="Options" :for="`v-${i}-opts`" hint="Comma-separated."><Input :id="`v-${i}-opts`" :model-value="(v.options ?? []).join(', ')" @update:model-value="v.options = String($event).split(',').map((s) => s.trim()).filter(Boolean)" /></FormField>
            <FormField v-else label="Pattern" :for="`v-${i}-pattern`" optional hint="Regex the value must match."><Input :id="`v-${i}-pattern`" :model-value="v.pattern ?? ''" class="font-mono" @update:model-value="v.pattern = String($event) || null" /></FormField>
            <div class="flex items-end gap-4 sm:col-span-4">
              <FormField label="Group" :for="`v-${i}-group`" optional class="w-40"><Input :id="`v-${i}-group`" :model-value="v.group ?? ''" placeholder="Network" @update:model-value="v.group = String($event) || null" /></FormField>
              <label class="flex items-center gap-2 pb-2 text-sm"><Checkbox :model-value="v.required" @update:model-value="v.required = $event === true" /> Required</label>
              <label class="flex items-center gap-2 pb-2 text-sm" title="Fixed by the template — shown to the operator, not editable"><Checkbox :model-value="!!v.preset" @update:model-value="v.preset = $event === true" /> Preset</label>
              <Button type="button" variant="ghost" size="sm" class="ml-auto text-muted-foreground hover:text-sev-error" @click="removeVariable(i)"><Trash2 class="size-4" /> Remove</Button>
            </div>
          </div>
          <Button type="button" variant="outline" size="sm" class="w-fit" @click="addVariable()"><Plus class="size-4" /> Add variable</Button>
        </TabsContent>

        <TabsContent value="requires" class="grid gap-3 sm:grid-cols-2">
          <FormField label="Architectures" for="req-arch" hint="Comma-separated: x86_64, aarch64."><Input id="req-arch" v-model="arch" class="font-mono" /></FormField>
          <FormField label="Minimum Docker" for="req-docker"><Input id="req-docker" :model-value="m.requires.min_docker ?? ''" class="font-mono" @update:model-value="m.requires.min_docker = String($event) || null" /></FormField>
          <FormField label="Minimum RAM (GB)" for="req-ram"><Input id="req-ram" v-model="ramGb" type="number" /></FormField>
          <FormField label="Minimum free disk (GB)" for="req-disk"><Input id="req-disk" v-model="diskGb" type="number" /></FormField>
          <div class="rounded-xl border border-border bg-card p-3 sm:col-span-2">
            <p class="text-sm font-medium text-foreground">Photon block</p>
            <p class="text-xs text-muted-foreground">What the render injects so anything deployed from this template is observable from its first second. This is the reason Photon deploys at all.</p>
            <div class="mt-2 grid gap-2 sm:grid-cols-2">
              <label class="flex items-center justify-between gap-3 text-sm" for="obs-endpoint">OTLP endpoint<Switch id="obs-endpoint" :model-value="m.observability.otlp_endpoint" @update:model-value="m.observability.otlp_endpoint = $event" /></label>
              <label class="flex items-center justify-between gap-3 text-sm" for="obs-token">Ingest token<Switch id="obs-token" :model-value="m.observability.ingest_token" @update:model-value="m.observability.ingest_token = $event" /></label>
              <label class="flex items-center justify-between gap-3 text-sm" for="obs-env">deployment.environment<Switch id="obs-env" :model-value="m.observability.environment" @update:model-value="m.observability.environment = $event" /></label>
              <FormField label="service.name override" for="obs-svc" optional hint="Leave empty to keep each compose service's own OTEL_SERVICE_NAME."><Input id="obs-svc" :model-value="m.observability.service_name ?? ''" class="font-mono" @update:model-value="m.observability.service_name = String($event) || null" /></FormField>
            </div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 sm:col-span-2">
            <p class="text-sm font-medium text-foreground">Health gate</p>
            <p class="text-xs text-muted-foreground">What "deployed successfully" means. Uses what Photon already has — a container health check, an uptime monitor, or an SLO burn-rate check — never a fourth health concept.</p>
            <div class="mt-2 grid gap-2 sm:grid-cols-[10rem_1fr_7rem]">
              <SelectMenu :model-value="m.health?.kind ?? ''" :options="[{ value: '', label: 'None' }, { value: 'container_healthy', label: 'Containers healthy' }, { value: 'uptime_monitor', label: 'Uptime monitor' }, { value: 'slo_burn', label: 'SLO burn rate' }]" content-class="w-48" size="md" aria-label="Health gate" @update:model-value="m.health = $event ? { kind: $event as any, detail: m.health?.detail ?? '', grace_secs: m.health?.grace_secs ?? 180 } : null" />
              <Input v-if="m.health" :model-value="m.health.detail" placeholder="which containers / monitor / objective" @update:model-value="m.health!.detail = String($event)" />
              <Input v-if="m.health" :model-value="m.health.grace_secs" type="number" placeholder="grace s" @update:model-value="m.health!.grace_secs = Number($event)" />
            </div>
          </div>
        </TabsContent>

        <TabsContent value="manifest">
          <pre class="max-h-[24rem] overflow-auto rounded-xl border border-border bg-card p-3 font-mono text-[11.5px] leading-5 text-foreground" data-testid="manifest-preview">{{ yaml }}</pre>
          <p class="mt-1 text-[11px] text-muted-foreground">photon.template.yaml — generated from the fields; stored with the compose file as this version.</p>
        </TabsContent>
      </Tabs>

      <p v-if="error" class="font-mono text-xs text-sev-error">{{ error }}</p>
      <div class="flex items-center justify-end gap-2">
        <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <GatedButton perm="templates" :disabled="!canSubmit || publish.isPending.value" data-testid="publish-submit" @click="submit"><Loader2 v-if="publish.isPending.value" class="animate-spin" /><Upload v-else class="size-4" /> Publish {{ m.version }}</GatedButton>
      </div>
    </DialogContent>
  </Dialog>
</template>
