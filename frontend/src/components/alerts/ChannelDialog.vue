<script setup lang="ts">
// Add/edit a notification channel. A preset Select reshapes the fields below it: Generic webhook
// (url/secret/headers), Discord (webhook URL), Telegram (bot token + chat id). A Test button sends
// a sample delivery through the draft-test route so the user can verify a preset before saving.
import { reactive, ref, watch, computed } from 'vue'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { StatusPill } from '@/components/ui/status-pill'
import { useCreateChannel, useUpdateChannel, useTestChannelDraft } from '@/lib/alertsQueries'
import type { AlertChannel, AlertChannelInput, ChannelKind, ChannelConfig, MutationResult } from '@/lib/core/api'

const props = defineProps<{ open: boolean; channel?: AlertChannel | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const KINDS: { value: ChannelKind; label: string }[] = [
  { value: 'webhook', label: 'Generic webhook' },
  { value: 'discord', label: 'Discord' },
  { value: 'telegram', label: 'Telegram' },
]

const blank = () => ({
  name: '',
  kind: 'webhook' as ChannelKind,
  url: '',
  secret: '',
  headersText: '',
  webhookUrl: '',
  botToken: '',
  chatId: '',
})
const form = reactive(blank())
const headersError = ref<string | null>(null)
const isEdit = computed(() => !!props.channel)

// Seed the form from a channel's config on open.
watch(
  () => props.channel,
  (c) => {
    Object.assign(form, blank())
    if (c) {
      form.name = c.name
      form.kind = c.kind
      const cfg = c.config
      if (cfg.type === 'webhook') {
        form.url = cfg.url
        form.secret = cfg.secret ?? ''
        form.headersText = cfg.headers && Object.keys(cfg.headers).length ? JSON.stringify(cfg.headers, null, 2) : ''
      } else if (cfg.type === 'discord') {
        form.webhookUrl = cfg.webhook_url
      } else if (cfg.type === 'telegram') {
        form.botToken = cfg.bot_token
        form.chatId = cfg.chat_id
      }
    }
    headersError.value = null
  },
  { immediate: true },
)
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
const testMut = useTestChannelDraft()
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

// Assemble the typed config for the current kind; returns null if headers JSON is invalid.
function buildConfig(): ChannelConfig | null {
  if (form.kind === 'discord') return { type: 'discord', webhook_url: form.webhookUrl.trim() }
  if (form.kind === 'telegram')
    return { type: 'telegram', bot_token: form.botToken.trim(), chat_id: form.chatId.trim() }
  const headers = parseHeaders()
  if (!headers.ok) return null
  return { type: 'webhook', url: form.url.trim(), secret: form.secret.trim() ? form.secret.trim() : null, headers: headers.value }
}

function buildInput(): AlertChannelInput | null {
  const config = buildConfig()
  if (!config) return null
  return { name: form.name.trim(), config }
}

function onSaved(res: MutationResult) {
  if (res.ok !== false) emit('update:open', false)
}

function submit() {
  const input = buildInput()
  if (!input) return
  if (isEdit.value && props.channel) {
    updateMut.mutate({ id: props.channel.id, input }, { onSuccess: onSaved })
  } else {
    createMut.mutate(input, { onSuccess: onSaved })
  }
}

function onTest() {
  const input = buildInput()
  if (!input) return
  testMut.mutate(input)
}

const testPill = computed<{ tone: 'success' | 'error' | 'neutral'; label: string } | null>(() => {
  if (testMut.isPending.value) return { tone: 'neutral', label: 'Testing…' }
  if (testMut.isSuccess.value && testMut.data.value) {
    return testMut.data.value.ok === false
      ? { tone: 'error', label: testMut.data.value.error ?? 'Failed' }
      : { tone: 'success', label: 'Delivered' }
  }
  return null
})
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-w-md">
      <DialogHeader>
        <DialogTitle>{{ isEdit ? 'Edit channel' : 'Add channel' }}</DialogTitle>
        <DialogDescription>
          Pick a channel type and fill in what it needs. Photon renders each alert in that provider's format.
        </DialogDescription>
      </DialogHeader>

      <form class="flex flex-col gap-4" @submit.prevent="submit">
        <FormField label="Name" for="channel-name">
          <Input id="channel-name" v-model="form.name" placeholder="#ops-webhook" required autocomplete="off" />
        </FormField>

        <FormField label="Type" for="channel-kind">
          <Select v-model="form.kind">
            <SelectTrigger id="channel-kind"><SelectValue placeholder="Select a channel type" /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="k in KINDS" :key="k.value" :value="k.value">{{ k.label }}</SelectItem>
            </SelectContent>
          </Select>
        </FormField>

        <!-- Generic webhook -->
        <template v-if="form.kind === 'webhook'">
          <FormField label="URL" for="channel-url" hint="The webhook endpoint Photon sends the alert payload to.">
            <Input id="channel-url" v-model="form.url" type="url" placeholder="https://hooks.example.com/services/…" required autocomplete="off" />
          </FormField>
          <FormField label="Secret" for="channel-secret" :optional="true" hint="Signs the payload with HMAC-SHA256 in the X-Photon-Signature header.">
            <Input id="channel-secret" v-model="form.secret" type="password" placeholder="whsec_…" autocomplete="off" />
          </FormField>
          <FormField label="Headers" for="channel-headers" :optional="true" :error="headersError ?? undefined" hint='Extra request headers as JSON, e.g. {"Authorization":"Bearer …"}'>
            <textarea id="channel-headers" v-model="form.headersText" rows="3" placeholder='{"Authorization":"Bearer …"}'
              class="flex w-full rounded-md border border-input bg-background px-3 py-1.5 font-mono text-xs shadow-sink transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring" />
          </FormField>
        </template>

        <!-- Discord -->
        <template v-else-if="form.kind === 'discord'">
          <FormField label="Webhook URL" for="channel-discord-url" hint="Server Settings → Integrations → Webhooks → Copy Webhook URL.">
            <Input id="channel-discord-url" v-model="form.webhookUrl" type="url" placeholder="https://discord.com/api/webhooks/…" required autocomplete="off" />
          </FormField>
        </template>

        <!-- Telegram -->
        <template v-else>
          <FormField label="Bot token" for="channel-tg-token" hint="Create a bot via @BotFather; it gives you a token like 123456:AA…">
            <Input id="channel-tg-token" v-model="form.botToken" type="password" placeholder="123456:AA…" required autocomplete="off" />
          </FormField>
          <FormField label="Chat ID" for="channel-tg-chat" hint="The target chat/channel id (e.g. -1001234567890).">
            <Input id="channel-tg-chat" v-model="form.chatId" placeholder="-1001234567890" required autocomplete="off" />
          </FormField>
        </template>

        <div class="flex items-center gap-2 pt-1">
          <Button type="button" variant="outline" size="sm" :disabled="testMut.isPending.value" @click="onTest">Test</Button>
          <StatusPill v-if="testPill" :tone="testPill.tone" class="truncate">{{ testPill.label }}</StatusPill>
          <div class="flex-1" />
          <Button type="button" variant="ghost" @click="emit('update:open', false)">Cancel</Button>
          <Button type="submit" :disabled="pending">
            {{ pending ? 'Saving…' : isEdit ? 'Save changes' : 'Add channel' }}
          </Button>
        </div>
      </form>
    </DialogContent>
  </Dialog>
</template>
