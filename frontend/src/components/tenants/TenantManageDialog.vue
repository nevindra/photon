<script setup lang="ts">
// Manage federation tenants (register / edit UI URL / rotate token / delete) — the tenant-registry
// twin of RumManageAppsDialog.vue: same list-with-rotate/delete-icon-buttons + create-form +
// minted-secret-panel-shown-once shape, except the minted value is a bearer TOKEN (not a public
// key) so the panel renders a copy-ready `[federation]` TOML snippet instead of an install one-
// liner. Mutations go through the tenantsQueries composables (Task 10), which already invalidate
// the tenants list + toast on the `{ ok, error }` result shape, so this component doesn't track its
// own error state and just fires the mutation.
import { ref, watch } from 'vue'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { Trash2, KeyRound, Plus } from 'lucide-vue-next'
import type { Tenant } from '@/lib/core/api'
import { useCreateTenant, useUpdateTenant, useRotateTenantToken, useDeleteTenant } from '@/lib/tenants/tenantsQueries'

const props = defineProps<{ open: boolean; tenants: Tenant[] }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const createMut = useCreateTenant()
const updateMut = useUpdateTenant()
const rotateMut = useRotateTenantToken()
const deleteMut = useDeleteTenant()

// Create form
const newName = ref('')
const newUiUrl = ref('')
const mintedToken = ref<string | null>(null)
const mintedFor = ref('')

async function submitCreate() {
  const name = newName.value.trim()
  if (!name) return
  const uiUrl = newUiUrl.value.trim()
  const res = await createMut.mutateAsync({ name, uiUrl: uiUrl || null })
  if (res.ok && res.token) {
    mintedToken.value = res.token
    mintedFor.value = name
    newName.value = ''
    newUiUrl.value = ''
  }
}

function editUiUrl(tenant: Tenant, value: string) {
  updateMut.mutate({ name: tenant.name, uiUrl: value.trim() || null })
}
async function rotate(tenant: Tenant) {
  const res = await rotateMut.mutateAsync(tenant.name)
  if (res.ok && res.token) {
    mintedToken.value = res.token
    mintedFor.value = tenant.name
  }
}
function remove(tenant: Tenant) {
  deleteMut.mutate(tenant.name)
}

// A copy-pasteable `[federation]` TOML snippet for the tenant just registered/rotated — the
// endpoint defaults to this photon's own origin, since that's the address a tenant reaches this
// (central) install at.
function fedSnippet(token: string): string {
  return `[federation]
endpoint = "${location.origin}"
token = "${token}"
mode = "summary"   # set to "full" to mirror raw telemetry`
}

// Clear the minted-token panel when the dialog closes, so reopening it doesn't show a stale
// "Token for X" panel from a previous session.
watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen) {
      mintedToken.value = null
      mintedFor.value = ''
    }
  },
)
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <!-- `[&>*]:min-w-0` lets the grid children shrink below their content's min-content width so
         long values (the TOML snippet, tokens) wrap/scroll inside the dialog instead of forcing it
         wider than the viewport (DialogContent is a fixed-width CSS grid). -->
    <DialogContent class="max-h-[85vh] max-w-2xl overflow-y-auto [&>*]:min-w-0">
      <DialogHeader>
        <DialogTitle>Manage tenants</DialogTitle>
        <DialogDescription>
          Register the Photon installs allowed to push federated telemetry here. The token is a secret — shown once.
        </DialogDescription>
      </DialogHeader>

      <!-- Existing tenants -->
      <p v-if="!tenants.length" class="py-4 text-center text-sm text-muted-foreground">No tenants registered yet.</p>
      <ul v-else class="flex max-h-72 flex-col divide-y divide-border overflow-y-auto rounded-md border border-border">
        <li v-for="tenant in tenants" :key="tenant.name" class="flex flex-col gap-2 p-3">
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <p class="text-sm font-medium text-foreground">{{ tenant.name }}</p>
              <p class="truncate font-mono text-xs text-muted-foreground" :title="tenant.token">{{ tenant.token }}</p>
            </div>
            <div class="flex shrink-0 gap-1">
              <Button
                variant="ghost"
                size="icon"
                class="size-7 text-muted-foreground hover:text-foreground"
                aria-label="Rotate token"
                title="Rotate token"
                @click="rotate(tenant)"
              >
                <KeyRound class="size-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="size-7 text-muted-foreground hover:text-sev-error"
                aria-label="Delete tenant"
                title="Delete tenant"
                @click="remove(tenant)"
              >
                <Trash2 class="size-4" />
              </Button>
            </div>
          </div>
          <div class="space-y-1">
            <label class="text-[11px] uppercase tracking-wide text-muted-foreground">UI URL (optional)</label>
            <input
              type="text"
              :data-testid="`tenant-ui-url-${tenant.name}`"
              :value="tenant.ui_url ?? ''"
              placeholder="https://tenant.example.com"
              class="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1.5 font-mono text-xs shadow-sink transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              @change="editUiUrl(tenant, ($event.target as HTMLInputElement).value)"
            />
          </div>
        </li>
      </ul>

      <!-- Create -->
      <form data-testid="new-tenant-form" class="flex flex-col gap-3 border-t border-border pt-4" @submit.prevent="submitCreate">
        <p class="text-sm font-medium text-foreground">Add tenant</p>
        <FormField label="Name" for="new-tenant-name" hint="Lowercase alphanumeric/hyphen — identifies the tenant everywhere.">
          <Input
            id="new-tenant-name"
            v-model="newName"
            data-testid="new-tenant-name"
            type="text"
            placeholder="divtik"
            autocomplete="off"
          />
        </FormField>
        <FormField label="UI URL" for="new-tenant-ui-url" hint="Optional — where this tenant's own Photon UI lives.">
          <Input
            id="new-tenant-ui-url"
            v-model="newUiUrl"
            data-testid="new-tenant-ui-url"
            type="text"
            placeholder="https://tenant.example.com"
            autocomplete="off"
          />
        </FormField>
        <Button type="submit" :disabled="createMut.isPending.value">
          <Plus class="size-4" /> {{ createMut.isPending.value ? 'Adding…' : 'Add tenant' }}
        </Button>
      </form>

      <!-- Minted token + install snippet -->
      <div v-if="mintedToken" class="rounded-lg border border-brand/40 bg-brand/5 p-3">
        <p class="text-xs font-medium text-foreground">
          Token for <span class="font-mono">{{ mintedFor }}</span> (shown once — copy it now):
        </p>
        <pre class="mt-2 whitespace-pre-wrap break-all rounded bg-surface-2 p-2 font-mono text-xs">{{ fedSnippet(mintedToken) }}</pre>
      </div>
    </DialogContent>
  </Dialog>
</template>
