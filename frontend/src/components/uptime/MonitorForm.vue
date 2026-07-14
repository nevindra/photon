<script setup lang="ts">
import { reactive, watch, computed } from 'vue'
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetFooter } from '@/components/ui/sheet'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { FormField } from '@/components/ui/form-field'
import { NumberField } from '@/components/ui/number-field'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

const props = defineProps({ modelValue: Boolean, monitor: { type: Object, default: null } })
const emit = defineEmits(['update:modelValue', 'save'])

const TYPES = [
  { value: 'http', label: 'HTTP(S)', hint: 'Request a URL and check the response' },
  { value: 'tcp', label: 'TCP', hint: 'Open a socket to a host and port' },
  { value: 'icmp', label: 'Ping (ICMP)', hint: 'Ping a host or IP address' },
]

const METHODS = ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS']

const blank = () => ({
  name: '',
  type: 'http',
  target: '',
  interval_secs: 60,
  timeout_secs: 10,
  retries: 3,
  http_method: 'GET',
  expect_status: '2xx',
  keyword: '',
  ignore_tls: false,
  follow_redirects: true,
  webhook_url: '',
})

const form = reactive(blank())

watch(
  () => props.monitor,
  (m) => {
    Object.assign(form, m ? { ...blank(), ...m } : blank())
  },
  { immediate: true },
)

const isHttp = computed(() => form.type === 'http')

const target = computed(() => {
  switch (form.type) {
    case 'tcp':
      return { placeholder: 'db.internal:5432', hint: 'Host and port to open a socket to.' }
    case 'icmp':
      return { placeholder: 'example.com or 10.0.0.1', hint: 'Host or IP address to ping.' }
    default:
      return { placeholder: 'https://example.com/health', hint: 'Full URL to request, including scheme.' }
  }
})

function submit() {
  const body = {
    name: form.name,
    type: form.type,
    target: form.target,
    interval_secs: Number(form.interval_secs),
    timeout_secs: Number(form.timeout_secs),
    retries: Number(form.retries),
    webhook_url: form.webhook_url || null,
  }
  if (isHttp.value) {
    Object.assign(body, {
      http_method: form.http_method,
      expect_status: form.expect_status,
      keyword: form.keyword || null,
      ignore_tls: form.ignore_tls,
      follow_redirects: form.follow_redirects,
    })
  }
  emit('save', body)
  emit('update:modelValue', false)
}
</script>

<template>
  <Sheet :open="modelValue" @update:open="emit('update:modelValue', $event)">
    <SheetContent side="right" class="flex w-[480px] flex-col gap-0 p-0 sm:max-w-[480px]">
      <form class="flex min-h-0 flex-1 flex-col" @submit.prevent="submit">
        <SheetHeader class="shrink-0 space-y-1 border-b border-border px-6 py-4 text-left">
          <SheetTitle>{{ monitor ? 'Edit monitor' : 'New monitor' }}</SheetTitle>
          <p class="text-sm text-muted-foreground">
            Photon checks this endpoint on a schedule and alerts you the moment it goes down.
          </p>
        </SheetHeader>

        <ScrollArea class="min-h-0 flex-1">
          <div class="space-y-7 px-6 py-5">
            <!-- General -->
            <section class="space-y-4">
              <p class="text-xs font-medium uppercase tracking-wider text-muted-foreground">General</p>

              <FormField label="Name" for="monitor-name">
                <Input id="monitor-name" v-model="form.name" placeholder="Checkout API" required />
              </FormField>

              <FormField label="Type" for="monitor-type">
                <Select v-model="form.type">
                  <SelectTrigger id="monitor-type">
                    <SelectValue placeholder="Select a check type" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="t in TYPES" :key="t.value" :value="t.value">
                      <span class="flex flex-col">
                        <span>{{ t.label }}</span>
                        <span class="text-xs text-muted-foreground">{{ t.hint }}</span>
                      </span>
                    </SelectItem>
                  </SelectContent>
                </Select>
              </FormField>

              <FormField label="Target" for="monitor-target" :hint="target.hint">
                <Input
                  id="monitor-target"
                  v-model="form.target"
                  required
                  :placeholder="target.placeholder"
                />
                <template #hint>
                  <p class="text-xs text-muted-foreground">{{ target.hint }}</p>
                </template>
              </FormField>
            </section>

            <!-- Schedule -->
            <section class="space-y-4">
              <p class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Schedule</p>

              <div class="grid grid-cols-3 gap-3">
                <div class="space-y-1.5">
                  <Label for="monitor-interval">Interval</Label>
                  <NumberField id="monitor-interval" v-model="form.interval_secs" :min="1" unit="s" />
                </div>
                <div class="space-y-1.5">
                  <Label for="monitor-timeout">Timeout</Label>
                  <NumberField id="monitor-timeout" v-model="form.timeout_secs" :min="1" unit="s" />
                </div>
                <div class="space-y-1.5">
                  <Label for="monitor-retries">Retries</Label>
                  <NumberField id="monitor-retries" v-model="form.retries" :min="0" />
                </div>
              </div>
              <p class="text-xs text-muted-foreground">
                How often to run the check, how long to wait for a reply, and how many failures in
                a row before the monitor is marked down.
              </p>
            </section>

            <!-- HTTP request -->
            <section v-if="isHttp" class="space-y-4">
              <p class="text-xs font-medium uppercase tracking-wider text-muted-foreground">HTTP request</p>

              <div class="grid grid-cols-2 gap-3">
                <FormField label="Method" for="monitor-method">
                  <Select v-model="form.http_method">
                    <SelectTrigger id="monitor-method">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="m in METHODS" :key="m" :value="m">{{ m }}</SelectItem>
                    </SelectContent>
                  </Select>
                </FormField>
                <FormField label="Expected status" for="monitor-expect-status" hint="true">
                  <Input id="monitor-expect-status" v-model="form.expect_status" placeholder="2xx" />
                  <template #hint>
                    <p class="text-xs text-muted-foreground">
                      A single code, a range, or a class — e.g. <code class="text-foreground">200</code>,
                      <code class="text-foreground">200-299</code>, or <code class="text-foreground">2xx</code>.
                    </p>
                  </template>
                </FormField>
              </div>

              <FormField
                label="Keyword"
                for="monitor-keyword"
                :optional="true"
                hint="Fail the check unless the response body contains this text."
              >
                <Input id="monitor-keyword" v-model="form.keyword" placeholder='e.g. "healthy"' />
              </FormField>

              <div class="space-y-2">
                <label
                  for="monitor-ignore-tls"
                  class="flex items-center justify-between gap-4 rounded-lg border border-border px-3 py-2.5"
                >
                  <span class="space-y-0.5">
                    <span class="block text-sm font-medium">Ignore TLS errors</span>
                    <span class="block text-xs text-muted-foreground">
                      Accept expired or self-signed certificates.
                    </span>
                  </span>
                  <Switch id="monitor-ignore-tls" v-model="form.ignore_tls" />
                </label>

                <label
                  for="monitor-follow-redirects"
                  class="flex items-center justify-between gap-4 rounded-lg border border-border px-3 py-2.5"
                >
                  <span class="space-y-0.5">
                    <span class="block text-sm font-medium">Follow redirects</span>
                    <span class="block text-xs text-muted-foreground">
                      Follow 3xx responses to their final destination.
                    </span>
                  </span>
                  <Switch id="monitor-follow-redirects" v-model="form.follow_redirects" />
                </label>
              </div>
            </section>

            <!-- Alerts -->
            <section class="space-y-4">
              <p class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Alerts</p>

              <FormField
                label="Webhook URL"
                for="monitor-webhook"
                :optional="true"
                hint="Photon POSTs a JSON payload here whenever this monitor changes state."
              >
                <Input id="monitor-webhook" v-model="form.webhook_url" placeholder="https://hooks.example.com/…" />
              </FormField>
            </section>
          </div>
        </ScrollArea>

        <SheetFooter class="shrink-0 border-t border-border px-6 py-4">
          <Button type="button" variant="ghost" @click="emit('update:modelValue', false)">Cancel</Button>
          <Button type="submit">{{ monitor ? 'Save changes' : 'Add monitor' }}</Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
