<script setup lang="ts">
// One notification channel in ChannelsGrid: a per-kind icon, masked endpoint, detail line, rule
// count, a session-local health pill (from this card's own Test click), and Test/Edit actions.
import { computed } from 'vue'
import { Webhook, Pencil, FlaskConical, MessageCircle, Send } from 'lucide-vue-next'
import { Card } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { StatusPill } from '@/components/ui/status-pill'
import { relative } from '@/lib/core/format'
import { useTestChannel } from '@/lib/alertsQueries'
import type { AlertChannel } from '@/lib/core/api'

const props = defineProps<{ channel: AlertChannel; ruleCount?: number }>()
const emit = defineEmits<{ edit: [id: string] }>()

const testMut = useTestChannel()
function onTest() {
  testMut.mutate(props.channel.id)
}

const icon = computed(() => {
  switch (props.channel.kind) {
    case 'discord':
      return MessageCircle
    case 'telegram':
      return Send
    default:
      return Webhook
  }
})

// The effective endpoint per kind, masked so tokens don't sit in plain view.
const endpoint = computed(() => {
  const cfg = props.channel.config
  if (cfg.type === 'discord') return cfg.webhook_url
  if (cfg.type === 'telegram') return 'api.telegram.org/bot•••/sendMessage'
  return cfg.url
})
const maskedUrl = computed(() => {
  const raw = endpoint.value
  try {
    const u = new URL(raw)
    const segments = u.pathname.split('/').filter(Boolean)
    const visible = segments.slice(0, 2).join('/')
    return `${u.protocol}//${u.host}${visible ? '/' + visible : ''}…`
  } catch {
    return raw.length > 34 ? `${raw.slice(0, 34)}…` : raw
  }
})

const detail = computed(() => {
  const cfg = props.channel.config
  if (cfg.type === 'discord') return 'Discord embed'
  if (cfg.type === 'telegram') return `Telegram · chat ${cfg.chat_id}`
  const parts = ['Generic JSON webhook']
  if (cfg.secret) parts.push('HMAC signed')
  const headerCount = Object.keys(cfg.headers ?? {}).length
  if (headerCount) parts.push(`${headerCount} custom header${headerCount === 1 ? '' : 's'}`)
  return parts.join(' · ')
})
const rulesLabel = computed(() => {
  const n = props.ruleCount ?? 0
  return `${n} rule${n === 1 ? '' : 's'}`
})

const health = computed<{ tone: 'success' | 'error' | 'neutral'; label: string }>(() => {
  if (testMut.isPending.value) return { tone: 'neutral', label: 'Testing…' }
  if (testMut.isSuccess.value && testMut.data.value) {
    return testMut.data.value.ok === false ? { tone: 'error', label: 'Failing' } : { tone: 'success', label: 'Healthy' }
  }
  return { tone: 'neutral', label: 'Untested' }
})

const lastDeliveryText = computed(() => {
  if (testMut.isPending.value) return 'Sending test…'
  if (testMut.isSuccess.value && testMut.submittedAt.value) {
    const ok = testMut.data.value?.ok !== false
    const when = relative(BigInt(testMut.submittedAt.value) * 1_000_000n)
    return ok ? `Test delivered ${when}` : `Test failed ${when}`
  }
  return 'No deliveries yet'
})
</script>

<template>
  <Card class="flex flex-col p-4">
    <div class="flex items-start gap-3">
      <div class="flex size-9 shrink-0 items-center justify-center rounded-md border border-brand/40 bg-brand/10 text-brand">
        <component :is="icon" class="size-4" />
      </div>
      <div class="min-w-0 flex-1">
        <div class="truncate font-medium text-foreground">{{ channel.name }}</div>
        <div class="truncate font-mono text-xs text-muted-foreground" :title="endpoint">{{ maskedUrl }}</div>
      </div>
      <StatusPill :tone="health.tone" class="shrink-0">{{ health.label }}</StatusPill>
    </div>

    <p class="mt-2.5 text-xs text-muted-foreground">{{ detail }} · {{ rulesLabel }}</p>

    <div class="mt-3 flex items-center gap-2 border-t border-border pt-3">
      <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">{{ lastDeliveryText }}</span>
      <Button
        variant="outline"
        size="sm"
        :disabled="testMut.isPending.value"
        @click="onTest"
      >
        <FlaskConical class="size-3.5" />
        Test
      </Button>
      <Button variant="outline" size="sm" @click="emit('edit', channel.id)">
        <Pencil class="size-3.5" />
        Edit
      </Button>
    </div>
  </Card>
</template>
