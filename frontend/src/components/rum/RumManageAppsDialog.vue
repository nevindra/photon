<script setup lang="ts">
// Manage RUM apps (register / edit origins / rotate key / delete) — a dialog launched from the RUM
// apps view. Apps are the browser apps allowed to POST beacons; `key` is a PUBLIC client identifier
// (safe to display) — the real auth boundary is the Origin allowlist, not the key. Mutations go
// through the rumQueries composables (Task 9), which already invalidate the apps list + toast on
// the `{ ok, error }` result shape — so, like MonitorDetailDialog/SettingsUsers, this component
// doesn't track its own error state and just fires the mutation.
import { ref, watch } from 'vue'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { Trash2, KeyRound, Plus } from 'lucide-vue-next'
import type { RumApp } from '@/lib/core/api'
import { useCreateRumApp, useUpdateRumApp, useRotateRumAppKey, useDeleteRumApp } from '@/lib/rum/rumQueries'

const props = defineProps<{ open: boolean; apps: RumApp[] }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const createMut = useCreateRumApp()
const updateMut = useUpdateRumApp()
const rotateMut = useRotateRumAppKey()
const deleteMut = useDeleteRumApp()

// Create form
const newName = ref('')
const newOrigins = ref('')
const mintedKey = ref<string | null>(null)
const mintedFor = ref('')

// Origins are entered one-per-line (or comma-separated) and split into the array the API wants.
function parseOrigins(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter(Boolean)
}

async function submitCreate() {
  const name = newName.value.trim()
  if (!name) return
  const origins = parseOrigins(newOrigins.value)
  const res = await createMut.mutateAsync({ name, input: { allowed_origins: origins } })
  if (res.ok && res.key) {
    mintedKey.value = res.key
    mintedFor.value = name
    newName.value = ''
    newOrigins.value = ''
  }
}

function editOrigins(app: RumApp, text: string) {
  updateMut.mutate({ name: app.name, input: { allowed_origins: parseOrigins(text) } })
}
function rotate(app: RumApp) {
  rotateMut.mutate(app.name)
}
function remove(app: RumApp) {
  deleteMut.mutate(app.name)
}

// A copy-pasteable install snippet for the app the user just registered.
function snippet(name: string, key: string): string {
  return `initPhoton({ app: '${name}', key: '${key}', endpoint: location.origin + '/api/rum' })`
}

// Clear the minted-key panel when the dialog closes, so reopening it doesn't show a stale
// "Key for X" panel from a previous session.
watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen) {
      mintedKey.value = null
      mintedFor.value = ''
    }
  },
)
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <!-- `[&>*]:min-w-0` lets the grid children shrink below their content's min-content width so
         long values (the install snippet, origins, keys) wrap/scroll inside the dialog instead of
         forcing it wider than the viewport (DialogContent is a fixed-width CSS grid). -->
    <DialogContent class="max-h-[85vh] max-w-2xl overflow-y-auto [&>*]:min-w-0">
      <DialogHeader>
        <DialogTitle>Manage RUM apps</DialogTitle>
        <DialogDescription>
          Register the frontend apps allowed to send beacons. The key is public — the Origin allowlist is the auth boundary.
        </DialogDescription>
      </DialogHeader>

      <!-- Existing apps -->
      <p v-if="!apps.length" class="py-4 text-center text-sm text-muted-foreground">No RUM apps registered yet.</p>
      <ul v-else class="flex max-h-72 flex-col divide-y divide-border overflow-y-auto rounded-md border border-border">
        <li v-for="app in apps" :key="app.name" class="flex flex-col gap-2 p-3">
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <p class="text-sm font-medium text-foreground">{{ app.name }}</p>
              <p class="truncate font-mono text-xs text-muted-foreground" :title="app.key">{{ app.key }}</p>
            </div>
            <div class="flex shrink-0 gap-1">
              <Button
                variant="ghost"
                size="icon"
                class="size-7 text-muted-foreground hover:text-foreground"
                aria-label="Rotate key"
                title="Rotate key"
                @click="rotate(app)"
              >
                <KeyRound class="size-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="size-7 text-muted-foreground hover:text-sev-error"
                aria-label="Delete app"
                title="Delete app"
                @click="remove(app)"
              >
                <Trash2 class="size-4" />
              </Button>
            </div>
          </div>
          <div class="space-y-1">
            <label class="text-[11px] uppercase tracking-wide text-muted-foreground">Allowed origins (one per line)</label>
            <textarea
              rows="2"
              data-testid="app-origins"
              :value="app.allowed_origins.join('\n')"
              class="flex w-full rounded-md border border-input bg-background px-3 py-1.5 font-mono text-xs shadow-sink transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              @change="editOrigins(app, ($event.target as HTMLTextAreaElement).value)"
            />
          </div>
        </li>
      </ul>

      <!-- Create -->
      <form class="flex flex-col gap-3 border-t border-border pt-4" @submit.prevent="submitCreate">
        <p class="text-sm font-medium text-foreground">Add app</p>
        <FormField label="Name" for="new-app-name" hint="Becomes the app's identifier (service.name).">
          <Input
            id="new-app-name"
            v-model="newName"
            data-testid="new-app-name"
            type="text"
            placeholder="web"
            autocomplete="off"
          />
        </FormField>
        <FormField
          label="Allowed origins"
          for="new-app-origins"
          hint="One origin per line (or comma-separated), e.g. https://app.example.com"
        >
          <textarea
            id="new-app-origins"
            v-model="newOrigins"
            data-testid="new-app-origins"
            rows="2"
            placeholder="https://app.example.com"
            class="flex w-full rounded-md border border-input bg-background px-3 py-1.5 font-mono text-xs shadow-sink transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          />
        </FormField>
        <Button type="submit" :disabled="createMut.isPending.value">
          <Plus class="size-4" /> {{ createMut.isPending.value ? 'Adding…' : 'Add app' }}
        </Button>
      </form>

      <!-- Minted key + install snippet -->
      <div v-if="mintedKey" class="rounded-lg border border-brand/40 bg-brand/5 p-3">
        <p class="text-xs font-medium text-foreground">
          Key for <span class="font-mono">{{ mintedFor }}</span> (shown once — it's public, but copy it now):
        </p>
        <pre class="mt-2 whitespace-pre-wrap break-all rounded bg-surface-2 p-2 font-mono text-xs">{{ snippet(mintedFor, mintedKey) }}</pre>
      </div>
    </DialogContent>
  </Dialog>
</template>
