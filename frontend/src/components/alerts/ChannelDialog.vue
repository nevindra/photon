<script setup lang="ts">
// Add/edit a notification channel — a small modal form (unlike MonitorForm's full-page Sheet,
// this only has four fields) that owns its own create/update mutations directly, mirroring
// RumManageAppsDialog's self-contained-mutation style. `channel: null` means create mode.
import { reactive, ref, watch, computed } from 'vue'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { useCreateChannel, useUpdateChannel } from '@/lib/alertsQueries'
import type { AlertChannel, AlertChannelInput, MutationResult } from '@/lib/core/api'

const props = defineProps<{ open: boolean; channel?: AlertChannel | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const blank = () => ({ name: '', url: '', secret: '', headersText: '' })
const form = reactive(blank())
const headersError = ref<string | null>(null)

const isEdit = computed(() => !!props.channel)

watch(
  () => props.channel,
  (c) => {
    Object.assign(
      form,
      c
        ? {
            name: c.name,
            url: c.url,
            secret: c.secret ?? '',
            headersText: c.headers && Object.keys(c.headers).length ? JSON.stringify(c.headers, null, 2) : '',
          }
        : blank(),
    )
    headersError.value = null
  },
  { immediate: true },
)

// Reset the form the moment the dialog closes so reopening it for "+ Add channel" right after
// editing another one doesn't flash the previous channel's stale values.
watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen && !props.channel) {
      Object.assign(form, blank())
      headersError.value = null
    }
  },
)

const createMut = useCreateChannel()
const updateMut = useUpdateChannel()
const pending = computed(() => createMut.isPending.value || updateMut.isPending.value)

function parseHeaders(): { ok: true; value: Record<string, string> | null } | { ok: false } {
  const text = form.headersText.trim()
  if (!text) {
    headersError.value = null
    return { ok: true, value: null }
  }
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    headersError.value = 'Invalid JSON.'
    return { ok: false }
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    headersError.value = 'Headers must be a JSON object of string values.'
    return { ok: false }
  }
  for (const v of Object.values(parsed as Record<string, unknown>)) {
    if (typeof v !== 'string') {
      headersError.value = 'Header values must be strings.'
      return { ok: false }
    }
  }
  headersError.value = null
  return { ok: true, value: parsed as Record<string, string> }
}

function onSaved(res: MutationResult) {
  if (res.ok !== false) emit('update:open', false)
}

function submit() {
  const headers = parseHeaders()
  if (!headers.ok) return

  const input: AlertChannelInput = {
    name: form.name.trim(),
    url: form.url.trim(),
    secret: form.secret.trim() ? form.secret.trim() : null,
    headers: headers.value,
  }

  if (isEdit.value && props.channel) {
    updateMut.mutate({ id: props.channel.id, input }, { onSuccess: onSaved })
  } else {
    createMut.mutate(input, { onSuccess: onSaved })
  }
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-w-md">
      <DialogHeader>
        <DialogTitle>{{ isEdit ? 'Edit channel' : 'Add channel' }}</DialogTitle>
        <DialogDescription>
          Photon POSTs a JSON payload here whenever a rule using this channel triggers or resolves.
        </DialogDescription>
      </DialogHeader>

      <form class="flex flex-col gap-4" @submit.prevent="submit">
        <FormField label="Name" for="channel-name">
          <Input id="channel-name" v-model="form.name" placeholder="#ops-webhook" required autocomplete="off" />
        </FormField>

        <FormField label="URL" for="channel-url" hint="The webhook endpoint Photon sends the alert payload to.">
          <Input
            id="channel-url"
            v-model="form.url"
            type="url"
            placeholder="https://hooks.example.com/services/…"
            required
            autocomplete="off"
          />
        </FormField>

        <FormField
          label="Secret"
          for="channel-secret"
          :optional="true"
          hint="Signs the payload with HMAC-SHA256 in the X-Photon-Signature header."
        >
          <Input id="channel-secret" v-model="form.secret" type="password" placeholder="whsec_…" autocomplete="off" />
        </FormField>

        <FormField
          label="Headers"
          for="channel-headers"
          :optional="true"
          :error="headersError ?? undefined"
          hint='Extra request headers as JSON, e.g. {"Authorization":"Bearer …"}'
        >
          <textarea
            id="channel-headers"
            v-model="form.headersText"
            rows="3"
            placeholder='{"Authorization":"Bearer …"}'
            class="flex w-full rounded-md border border-input bg-background px-3 py-1.5 font-mono text-xs shadow-sink transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          />
        </FormField>

        <div class="flex items-center justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" @click="emit('update:open', false)">Cancel</Button>
          <Button type="submit" :disabled="pending">
            {{ pending ? 'Saving…' : isEdit ? 'Save changes' : 'Add channel' }}
          </Button>
        </div>
      </form>
    </DialogContent>
  </Dialog>
</template>
