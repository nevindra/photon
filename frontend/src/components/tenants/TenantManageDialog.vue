<script setup lang="ts">
// Add/edit dialog for one federation tenant. `tenant` prop null → add mode (name + UI URL, minted
// token shown once on success); non-null → edit mode (UI URL + rotate token, rotated token shown
// once the same way). Row-level delete lives in the table (DataTenants), not here. Mutations go
// through the tenantsQueries composables, which invalidate the tenants list + toast on the
// `{ ok, error }` result shape, so this component doesn't track its own error state.
import { ref, computed, watch } from 'vue'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { KeyRound, Plus, Copy } from 'lucide-vue-next'
import type { Tenant } from '@/lib/core/api'
import { useCreateTenant, useUpdateTenant, useRotateTenantToken } from '@/lib/tenants/tenantsQueries'
import { useCopy } from '@/lib/core/useCopy'

const props = defineProps<{ open: boolean; tenant: Tenant | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const createMut = useCreateTenant()
const updateMut = useUpdateTenant()
const rotateMut = useRotateTenantToken()

const isEdit = computed(() => props.tenant !== null)
const { copy } = useCopy()

const name = ref('')
const uiUrl = ref('')
// Mode is the TENANT's own choice (`[federation] mode` in its config file — central is push-only
// and can't set it remotely). This picker exists for discoverability: it drives the generated
// snippet below so users learn both modes exist.
const mode = ref<'summary' | 'full' | 'full-traces'>('summary')
const mintedToken = ref<string | null>(null)
const mintedFor = ref('')

// Re-seed the form each open: edit mode from the tenant row, add mode blank. Also clears any
// minted-token panel from a previous open so a stale secret never re-renders.
watch(
  () => props.open,
  (isOpen) => {
    if (isOpen) {
      name.value = props.tenant?.name ?? ''
      uiUrl.value = props.tenant?.ui_url ?? ''
      mode.value = 'summary'
    } else {
      mintedToken.value = null
      mintedFor.value = ''
    }
  },
)

async function submit() {
  if (isEdit.value) {
    const res = await updateMut.mutateAsync({ name: props.tenant!.name, uiUrl: uiUrl.value.trim() || null })
    if (res.ok && !mintedToken.value) emit('update:open', false)
    return
  }
  const trimmed = name.value.trim()
  if (!trimmed) return
  const res = await createMut.mutateAsync({ name: trimmed, uiUrl: uiUrl.value.trim() || null })
  if (res.ok && res.token) {
    mintedToken.value = res.token
    mintedFor.value = trimmed
  }
}

async function rotate() {
  if (!props.tenant) return
  const res = await rotateMut.mutateAsync(props.tenant.name)
  if (res.ok && res.token) {
    mintedToken.value = res.token
    mintedFor.value = props.tenant.name
  }
}

// A copy-pasteable `[federation]` TOML snippet for the token just minted. The endpoint must be
// central's OTLP *ingest* base URL (`:4318` by default) — NOT this UI's origin, which serves the
// SPA and would swallow pushes with a 200 — so emit a placeholder host + the default ingest port.
function fedSnippet(token: string): string {
  const lines = [
    '[federation]',
    `endpoint = "${location.protocol}//${location.hostname}:4318"   # central's OTLP ingest URL, not the UI origin`,
    `token = "${token}"`,
  ]
  if (mode.value === 'summary') {
    lines.push('mode = "summary"   # health summary only; set to "full" to mirror raw telemetry')
  } else if (mode.value === 'full') {
    lines.push('mode = "full"   # mirrors raw logs/traces/metrics to central (plus the health summary)')
  } else {
    lines.push('mode = "full"')
    lines.push('signals = ["traces"]   # mirrors traces only — services/APM without logs')
  }
  return lines.join('\n')
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <!-- `[&>*]:min-w-0` lets the grid children shrink below their content's min-content width so
         long values (the TOML snippet, tokens) wrap/scroll inside the dialog instead of forcing it
         wider than the viewport (DialogContent is a fixed-width CSS grid). -->
    <DialogContent class="max-h-[85vh] max-w-2xl overflow-y-auto [&>*]:min-w-0">
      <DialogHeader>
        <DialogTitle>{{ isEdit ? `Edit ${tenant!.name}` : 'Add tenant' }}</DialogTitle>
        <DialogDescription>
          {{ isEdit
            ? 'Update this tenant, or rotate its push token. A rotated token is a secret — shown once.'
            : 'Register a Photon install allowed to push federated telemetry here. The token is a secret — shown once.' }}
        </DialogDescription>
      </DialogHeader>

      <form data-testid="tenant-form" class="flex flex-col gap-3" @submit.prevent="submit">
        <FormField v-if="!isEdit" label="Name" for="tenant-name" hint="Lowercase alphanumeric/hyphen — identifies the tenant everywhere.">
          <Input
            id="tenant-name"
            v-model="name"
            data-testid="tenant-name"
            type="text"
            placeholder="acme-corp"
            autocomplete="off"
          />
        </FormField>
        <FormField label="UI URL" for="tenant-ui-url" hint="Optional — where this tenant's own Photon UI lives.">
          <Input
            id="tenant-ui-url"
            v-model="uiUrl"
            data-testid="tenant-ui-url"
            type="text"
            placeholder="https://tenant.example.com"
            autocomplete="off"
          />
        </FormField>
        <FormField
          label="Mode"
          for="tenant-mode"
          hint="Set in the tenant's own config file — this choice only pre-fills the snippet below. Summary pushes health metrics only; full mirrors the tenant's raw telemetry here; traces-only mirrors just spans (services/APM without logs)."
        >
          <Segmented id="tenant-mode" v-model="mode" data-testid="tenant-mode">
            <SegmentedItem value="summary">Summary</SegmentedItem>
            <SegmentedItem value="full">Full</SegmentedItem>
            <SegmentedItem value="full-traces">Traces only</SegmentedItem>
          </Segmented>
        </FormField>
        <div class="flex items-center gap-2">
          <Button type="submit" :disabled="createMut.isPending.value || updateMut.isPending.value">
            <template v-if="isEdit">{{ updateMut.isPending.value ? 'Saving…' : 'Save' }}</template>
            <template v-else><Plus class="size-4" /> {{ createMut.isPending.value ? 'Adding…' : 'Add tenant' }}</template>
          </Button>
          <Button
            v-if="isEdit"
            type="button"
            variant="outline"
            data-testid="tenant-rotate"
            :disabled="rotateMut.isPending.value"
            @click="rotate"
          >
            <KeyRound class="size-4" /> {{ rotateMut.isPending.value ? 'Rotating…' : 'Rotate token' }}
          </Button>
        </div>
      </form>

      <!-- Minted token + install snippet -->
      <div v-if="mintedToken" class="rounded-lg border border-brand/40 bg-brand/5 p-3">
        <div class="flex items-center justify-between gap-2">
          <p class="text-xs font-medium text-foreground">
            Token for <span class="font-mono">{{ mintedFor }}</span> (shown once — copy it now):
          </p>
          <div class="flex shrink-0 gap-1">
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="h-7 px-2 text-xs"
              data-testid="tenant-copy-token"
              @click="copy(mintedToken, 'token')"
            >
              <Copy class="size-3.5" /> Token
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="h-7 px-2 text-xs"
              data-testid="tenant-copy-snippet"
              @click="copy(fedSnippet(mintedToken), 'config snippet')"
            >
              <Copy class="size-3.5" /> Snippet
            </Button>
          </div>
        </div>
        <pre class="mt-2 whitespace-pre-wrap break-all rounded bg-surface-2 p-2 font-mono text-xs">{{ fedSnippet(mintedToken) }}</pre>
      </div>
    </DialogContent>
  </Dialog>
</template>
