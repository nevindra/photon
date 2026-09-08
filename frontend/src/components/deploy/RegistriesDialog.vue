<script setup lang="ts">
// Template sources: the local catalog is implicit; remote registries by URL (ours, or Portainer
// v2/v3 JSON) sync on a schedule, and a Portainer JSON file can be pasted and imported once
// (R5.7) — several hundred community templates for free. The third tab is a different kind of
// registry: IMAGE registries and the credentials hosts pull private images with (a GitLab
// deploy token, a GHCR PAT). Stored encrypted, write-only, handed to an agent only for the
// pull at hand — nothing is ever `docker login`-ed on a host.
import { computed, ref } from 'vue'
import { Plus, RefreshCw, Trash2, Loader2, Import, KeyRound } from 'lucide-vue-next'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { FormField } from '@/components/ui/form-field'
import { SelectMenu } from '@/components/ui/select-menu'
import { StatusPill } from '@/components/ui/status-pill'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import GatedButton from './GatedButton.vue'
import { useRegistries, useAddRegistry, useSyncRegistry, useDeleteRegistry, useImportPortainer, useImageRegistries, useAddImageRegistry, useUpdateImageRegistry, useDeleteImageRegistry } from '@/lib/deploy/deployQueries'
import { parsePortainerTemplates } from '@/lib/deploy/portainer'
import { ageMs } from '@/lib/deploy/audit'
import { scopeText } from '@/lib/deploy/templates'
import type { Registry, RegistryCredential, TargetSelector } from '@/lib/core/api'

defineProps<{ open: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const q = useRegistries()
const registries = computed(() => q.data.value?.registries ?? [])
const add = useAddRegistry()
const sync = useSyncRegistry()
const del = useDeleteRegistry()
const imp = useImportPortainer()

const mode = ref<'registries' | 'import' | 'credentials'>('registries')
const name = ref('')
const url = ref('')
const kind = ref<Registry['kind']>('portainer_v3')
const KINDS = [{ value: 'photon', label: 'Photon registry' }, { value: 'portainer_v3', label: 'Portainer v3 JSON' }, { value: 'portainer_v2', label: 'Portainer v2 JSON' }]
async function doAdd(): Promise<void> {
  const res = await add.mutateAsync({ name: name.value, url: url.value, kind: kind.value })
  if (res.ok) {
    name.value = ''
    url.value = ''
  }
}

const json = ref('')
const source = ref('pasted')
const preview = computed(() => {
  if (!json.value.trim()) return null
  try {
    const r = parsePortainerTemplates(json.value, source.value)
    return { ok: true as const, imported: r.imported.length, skipped: r.skipped.length, version: r.version, names: r.imported.slice(0, 6).map((t) => t.manifest.name) }
  } catch (e) {
    return { ok: false as const, error: (e as Error).message }
  }
})
async function doImport(): Promise<void> {
  const res = await imp.mutateAsync({ json: json.value, source: source.value })
  if (res.ok) json.value = ''
}
const STATUS = { ok: 'success', error: 'error', syncing: 'info' } as const

// --- Image registry credentials ---
const credsQ = useImageRegistries()
const credentials = computed(() => credsQ.data.value?.credentials ?? [])
const addCred = useAddImageRegistry()
const updateCred = useUpdateImageRegistry()
const delCred = useDeleteImageRegistry()
const cServer = ref('')
const cUser = ref('')
const cSecret = ref('')
const cLabel = ref('')
const cScopeKind = ref<'all' | 'site' | 'hosts'>('all')
const cScopeValue = ref('')
const SCOPES = [{ value: 'all', label: 'Every host' }, { value: 'site', label: 'Hosts in a site' }, { value: 'hosts', label: 'Named hosts' }]
// Rotating: the row whose token is being replaced (the one edit that needs a field).
const rotating = ref<RegistryCredential | null>(null)
const rotateSecret = ref('')
function buildScope(): TargetSelector | null {
  const parts = cScopeValue.value.split(',').map((v) => v.trim()).filter(Boolean)
  if (cScopeKind.value === 'site') return { kind: 'site', sites: parts }
  if (cScopeKind.value === 'hosts') return { kind: 'hosts', hosts: parts }
  return null
}
const credValid = computed(() => cServer.value.trim() !== '' && cUser.value.trim() !== '' && cSecret.value.trim() !== '' && (cScopeKind.value === 'all' || cScopeValue.value.trim() !== ''))
async function doAddCred(): Promise<void> {
  const res = await addCred.mutateAsync({ server: cServer.value, username: cUser.value, secret: cSecret.value, label: cLabel.value || null, scope: buildScope() })
  if (res.ok) {
    cServer.value = ''
    cUser.value = ''
    cSecret.value = ''
    cLabel.value = ''
    cScopeKind.value = 'all'
    cScopeValue.value = ''
  }
}
function startRotate(c: RegistryCredential): void {
  rotating.value = c
  rotateSecret.value = ''
}
function cancelRotate(): void {
  rotating.value = null
  rotateSecret.value = ''
}
async function doRotate(): Promise<void> {
  if (!rotating.value || !rotateSecret.value.trim()) return
  const res = await updateCred.mutateAsync({ id: rotating.value.id, input: { secret: rotateSecret.value } })
  if (res.ok) cancelRotate()
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-h-[88vh] max-w-2xl overflow-y-auto [&>*]:min-w-0" data-testid="registries-dialog">
      <DialogHeader>
        <DialogTitle>Template sources</DialogTitle>
        <DialogDescription>The local catalog is where you publish. Registries add templates by URL and re-sync; a Portainer JSON file can be imported once. Image registries hold what hosts pull private images with.</DialogDescription>
      </DialogHeader>
      <Segmented v-model="mode" class="w-fit">
        <SegmentedItem value="registries">Registries</SegmentedItem>
        <SegmentedItem value="import">Import Portainer JSON</SegmentedItem>
        <SegmentedItem value="credentials" data-testid="mode-credentials">Image registries</SegmentedItem>
      </Segmented>

      <template v-if="mode === 'registries'">
        <ul class="flex flex-col divide-y divide-border overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="registries-list">
          <li v-for="r in registries" :key="r.id" class="flex flex-wrap items-center gap-3 px-4 py-3" :data-registry="r.id">
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-2"><span class="font-medium text-foreground">{{ r.name }}</span><StatusPill :tone="STATUS[r.status]" :dot="r.status !== 'syncing'"><Loader2 v-if="r.status === 'syncing'" class="animate-spin" />{{ r.status === 'syncing' ? 'Syncing' : r.status === 'ok' ? 'Synced' : 'Error' }}</StatusPill><span class="text-xs text-muted-foreground">{{ r.kind.replace('_', ' ') }}</span></div>
              <div class="truncate font-mono text-[11px] text-muted-foreground" :title="r.url">{{ r.url }}</div>
              <div class="text-xs text-muted-foreground">{{ r.template_count }} templates · {{ r.last_sync_ms ? `synced ${ageMs(r.last_sync_ms)}` : 'never synced' }}<span v-if="r.error" class="text-sev-error"> · {{ r.error }}</span></div>
            </div>
            <GatedButton perm="templates" size="sm" variant="outline" class="h-7 px-2 text-xs" :disabled="r.status === 'syncing'" @click="sync.mutate(r.id)"><RefreshCw class="size-3.5" /> Sync</GatedButton>
            <GatedButton perm="templates" size="icon" variant="ghost" class="size-7 text-muted-foreground hover:text-sev-error" :aria-label="`Remove ${r.name}`" @click="del.mutate(r.id)"><Trash2 class="size-4" /></GatedButton>
          </li>
          <li v-if="!registries.length" class="px-4 py-6 text-center text-sm text-muted-foreground">No registries yet.</li>
        </ul>
        <form class="grid gap-3 sm:grid-cols-[1fr_1.5fr_auto]" @submit.prevent="doAdd">
          <FormField label="Name" for="reg-name"><Input id="reg-name" v-model="name" placeholder="Community" autocomplete="off" /></FormField>
          <FormField label="URL" for="reg-url"><Input id="reg-url" v-model="url" placeholder="https://…/templates.json" class="font-mono" autocomplete="off" /></FormField>
          <FormField label="Kind" for="reg-kind"><SelectMenu id="reg-kind" :model-value="kind" :options="KINDS" content-class="w-48" size="md" @update:model-value="kind = $event as Registry['kind']" /></FormField>
          <div class="sm:col-span-3 flex justify-end"><GatedButton perm="templates" type="submit" size="sm" :disabled="add.isPending.value"><Plus class="size-4" /> Add registry</GatedButton></div>
        </form>
      </template>

      <template v-else-if="mode === 'credentials'">
        <p class="text-xs text-muted-foreground">What a host presents when it pulls a private image — a GitLab deploy token (<code class="font-mono">read_registry</code>), a GHCR token, a Hub login. Kept encrypted here and handed to the agent for the pull only; the token is never shown again and no host is <code class="font-mono">docker login</code>-ed. Preflight names the credential each image will use.</p>
        <ul class="flex flex-col divide-y divide-border overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="credentials-list">
          <li v-for="c in credentials" :key="c.id" class="flex flex-wrap items-center gap-3 px-4 py-3" :data-credential="c.id">
            <KeyRound class="size-4 shrink-0 text-muted-foreground" />
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-2"><span class="font-mono text-sm font-medium text-foreground">{{ c.server }}</span><span class="text-xs text-muted-foreground">as <span class="font-mono">{{ c.username }}</span></span></div>
              <div class="text-xs text-muted-foreground"><span v-if="c.label">{{ c.label }} · </span>{{ scopeText(c.scope) }} · updated {{ ageMs(c.updated_ms) }} by {{ c.created_by }}</div>
              <form v-if="rotating?.id === c.id" class="mt-2 flex flex-wrap items-end gap-2" @submit.prevent="doRotate">
                <FormField label="New token" :for="`rotate-${c.id}`" class="min-w-[16rem] flex-1"><Input :id="`rotate-${c.id}`" v-model="rotateSecret" type="password" autocomplete="new-password" class="font-mono" /></FormField>
                <GatedButton perm="templates" type="submit" size="sm" :disabled="!rotateSecret.trim() || updateCred.isPending.value">Replace</GatedButton>
                <Button type="button" size="sm" variant="ghost" @click="cancelRotate">Cancel</Button>
              </form>
            </div>
            <GatedButton v-if="rotating?.id !== c.id" perm="templates" size="sm" variant="outline" class="h-7 px-2 text-xs" @click="startRotate(c)"><RefreshCw class="size-3.5" /> Rotate token</GatedButton>
            <GatedButton perm="templates" size="icon" variant="ghost" class="size-7 text-muted-foreground hover:text-sev-error" :aria-label="`Remove credential for ${c.server}`" @click="delCred.mutate(c.id)"><Trash2 class="size-4" /></GatedButton>
          </li>
          <li v-if="!credentials.length" class="px-4 py-6 text-center text-sm text-muted-foreground">No registry credentials yet. Hosts pull anonymously — public images only.</li>
        </ul>
        <form class="grid gap-3 sm:grid-cols-2" data-testid="credential-form" @submit.prevent="doAddCred">
          <FormField label="Registry" for="cred-server" hint="The host an image reference names."><Input id="cred-server" v-model="cServer" placeholder="registry.gitlab.com" class="font-mono" autocomplete="off" /></FormField>
          <FormField label="Label" for="cred-label" hint="Optional — what this token is."><Input id="cred-label" v-model="cLabel" placeholder="GitLab deploy token, nodefluxio group" autocomplete="off" /></FormField>
          <FormField label="Username" for="cred-user" hint="For a GitLab deploy token, the token's own username."><Input id="cred-user" v-model="cUser" class="font-mono" autocomplete="off" /></FormField>
          <FormField label="Token or password" for="cred-secret" hint="Stored encrypted; never shown again."><Input id="cred-secret" v-model="cSecret" type="password" class="font-mono" autocomplete="new-password" /></FormField>
          <FormField label="Hosts that may use it" for="cred-scope"><SelectMenu id="cred-scope" :model-value="cScopeKind" :options="SCOPES" content-class="w-48" size="md" @update:model-value="cScopeKind = $event as 'all' | 'site' | 'hosts'" /></FormField>
          <FormField v-if="cScopeKind !== 'all'" :label="cScopeKind === 'site' ? 'Sites' : 'Host names'" for="cred-scope-value" hint="Comma-separated."><Input id="cred-scope-value" v-model="cScopeValue" class="font-mono" autocomplete="off" /></FormField>
          <div class="sm:col-span-2 flex justify-end"><GatedButton perm="templates" type="submit" size="sm" :disabled="!credValid || addCred.isPending.value" data-testid="credential-submit"><Plus class="size-4" /> Store credential</GatedButton></div>
        </form>
      </template>

      <template v-else>
        <FormField label="Portainer app-template JSON (v2 or v3)" for="import-json" hint="Compose stacks and containers become templates; Swarm stacks are skipped. Paste the file's contents.">
          <Textarea id="import-json" v-model="json" mono class="min-h-[10rem]" placeholder='{"version":"3","templates":[…]}' data-testid="import-json" />
        </FormField>
        <div class="grid gap-3 sm:grid-cols-[1fr_auto] sm:items-end">
          <FormField label="Source name" for="import-source" hint="Recorded on each imported template."><Input id="import-source" v-model="source" autocomplete="off" /></FormField>
          <GatedButton perm="templates" :disabled="!preview?.ok || imp.isPending.value" data-testid="import-submit" @click="doImport"><Loader2 v-if="imp.isPending.value" class="animate-spin" /><Import v-else class="size-4" /> Import {{ preview?.ok ? preview.imported : '' }}</GatedButton>
        </div>
        <p v-if="preview" class="text-xs" :class="preview.ok ? 'text-muted-foreground' : 'text-sev-error'" data-testid="import-preview">
          <template v-if="preview.ok">Portainer v{{ preview.version }} · {{ preview.imported }} to import{{ preview.skipped ? `, ${preview.skipped} skipped` : '' }}: <span class="text-foreground">{{ preview.names.join(', ') }}{{ preview.imported > 6 ? '…' : '' }}</span></template>
          <template v-else>{{ preview.error }}</template>
        </p>
      </template>
    </DialogContent>
  </Dialog>
</template>
