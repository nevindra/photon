<script setup lang="ts">
// The create/edit alert-rule dialog (Task 14): name/description → ConditionBuilder (signal +
// condition + live preview) → channel multi-select → severity + for. Self-contained mutations
// (useCreateRule/useUpdateRule/useTestRule), mirroring ChannelDialog.vue's identical shape —
// `v-model:open` + an optional `rule` prop (`null`/absent = create mode), closing itself on a
// successful save. `ConditionBuilder` is `:key`-ed on the target rule's id so it reseeds cleanly
// from a fresh `condition` prop whenever the dialog switches which rule it's editing (see that
// component's header comment for why it doesn't use an ongoing prop watcher instead).
import { computed, reactive, ref, watch } from 'vue'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField } from '@/components/ui/form-field'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { SelectMenu } from '@/components/ui/select-menu'
import ConditionBuilder from './ConditionBuilder.vue'
import { toast } from '@/components/ui/toast'
import { useCreateRule, useUpdateRule, useTestRule, useChannels } from '@/lib/alertsQueries'
import type { AlertRule, AlertRuleInput, AlertCondition, AlertSeverity, MutationResult } from '@/lib/core/api'

const props = defineProps<{ open: boolean; rule?: AlertRule | null; seed?: AlertRuleInput | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const isEdit = computed(() => !!props.rule)
const createNonce = ref(0)

const SEVERITIES: { value: AlertSeverity; label: string }[] = [
  { value: 'info', label: 'info' },
  { value: 'warning', label: 'warning' },
  { value: 'critical', label: 'critical' },
]
const FOR_OPTIONS = [
  { value: 0, label: 'immediately' },
  { value: 60, label: '1m' },
  { value: 300, label: '5m' },
  { value: 600, label: '10m' },
  { value: 900, label: '15m' },
  { value: 1800, label: '30m' },
]

function defaultCondition(): AlertCondition {
  return { signal: 'metrics', metric_name: '', agg: 'avg', window_secs: 300, cmp: 'gt', threshold: 0.9 }
}

const blank = () => ({
  name: '',
  description: '',
  severity: 'warning' as AlertSeverity,
  for_secs: 300,
  channel_ids: [] as string[],
})

const form = reactive(blank())
const condition = ref<AlertCondition | null>(null)

function applyCreateDraft() {
  // create mode: pre-fill from `seed` if present, else blank
  const s = props.seed
  Object.assign(form, {
    name: s?.name ?? '',
    description: s?.description ?? '',
    severity: s?.severity ?? ('warning' as AlertSeverity),
    for_secs: s?.for_secs ?? 300,
    channel_ids: s?.channel_ids ? [...s.channel_ids] : [],
  })
  condition.value = s?.condition ?? defaultCondition()
}

watch(
  () => props.rule,
  (r) => {
    if (r) {
      Object.assign(form, {
        name: r.name,
        description: r.description ?? '',
        severity: r.severity,
        for_secs: r.for_secs,
        channel_ids: [...r.channel_ids],
      })
      condition.value = r.condition
    } else {
      applyCreateDraft()
    }
  },
  { immediate: true },
)

watch(
  () => props.open,
  (isOpen) => {
    if (isOpen && !props.rule) {
      applyCreateDraft() // re-apply seed each open so a new template draft takes effect
      createNonce.value++ // force ConditionBuilder to remount + reseed from the new condition
    } else if (!isOpen && !props.rule) {
      Object.assign(form, blank())
      condition.value = defaultCondition()
    }
  },
)

const channelsQuery = useChannels()
const channels = computed(() => channelsQuery.data.value ?? [])

function toggleChannel(id: string) {
  const i = form.channel_ids.indexOf(id)
  if (i === -1) form.channel_ids.push(id)
  else form.channel_ids.splice(i, 1)
}

function setSeverity(v: unknown) {
  if (typeof v === 'string' && v) form.severity = v as AlertSeverity
}

const conditionBuilderRef = ref<InstanceType<typeof ConditionBuilder> | null>(null)
const conditionValid = computed(() => conditionBuilderRef.value?.isValid ?? false)
const canSubmit = computed(() => form.name.trim().length > 0 && conditionValid.value)

const createMut = useCreateRule()
const updateMut = useUpdateRule()
const testMut = useTestRule()
const pending = computed(() => createMut.isPending.value || updateMut.isPending.value)

function onSaved(res: MutationResult) {
  if (res.ok !== false) emit('update:open', false)
}

function submit() {
  if (!canSubmit.value || !condition.value) return
  const input: AlertRuleInput = {
    name: form.name.trim(),
    description: form.description.trim() ? form.description.trim() : null,
    signal: condition.value.signal,
    condition: condition.value,
    for_secs: form.for_secs,
    severity: form.severity,
    channel_ids: form.channel_ids,
  }
  if (isEdit.value && props.rule) {
    updateMut.mutate({ id: props.rule.id, input }, { onSuccess: onSaved })
  } else {
    createMut.mutate(input, { onSuccess: onSaved })
  }
}

// A saved rule tests itself server-side (evaluates the persisted condition). An unsaved draft has
// no id to test — fall back to summarizing the ConditionBuilder's own live preview, worded exactly
// like useTestRule's toast for a consistent "Test now" experience either way.
function testNow() {
  if (props.rule?.id) {
    testMut.mutate(props.rule.id)
    return
  }
  const series = conditionBuilderRef.value?.previewSeries ?? []
  const n = series.filter((s) => s.breaching).length
  toast({ variant: 'success', title: n ? `Would trigger on ${n} series` : 'No series would trigger now' })
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-h-[85vh] max-w-2xl overflow-y-auto">
      <DialogHeader>
        <DialogTitle>{{ isEdit ? 'Edit alert' : 'New alert' }}</DialogTitle>
        <DialogDescription>Watch a signal and send a webhook when the condition holds.</DialogDescription>
      </DialogHeader>

      <form class="flex flex-col gap-5" @submit.prevent="submit">
        <FormField label="Name" for="rule-name">
          <Input id="rule-name" v-model="form.name" placeholder="Checkout error rate high" required autocomplete="off" />
        </FormField>

        <FormField label="Description" for="rule-description" :optional="true">
          <Input
            id="rule-description"
            v-model="form.description"
            placeholder="What this rule watches for…"
            autocomplete="off"
          />
        </FormField>

        <div class="space-y-1.5">
          <span class="text-xs font-medium uppercase tracking-wider text-muted-foreground">Condition</span>
          <ConditionBuilder :key="rule?.id ?? `new-${createNonce}`" ref="conditionBuilderRef" v-model:condition="condition" />
        </div>

        <FormField label="Notify" hint="Where the webhook is sent when this rule triggers or resolves.">
          <div class="flex flex-wrap gap-2">
            <button
              v-for="c in channels"
              :key="c.id"
              type="button"
              :aria-pressed="form.channel_ids.includes(c.id)"
              class="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
              :class="
                form.channel_ids.includes(c.id)
                  ? 'border-brand/40 bg-brand/10 text-brand'
                  : 'border-border bg-muted text-muted-foreground hover:text-foreground'
              "
              @click="toggleChannel(c.id)"
            >
              {{ c.name }}
            </button>
            <p v-if="!channels.length" class="text-xs text-muted-foreground">
              No channels yet — add one from the Channels tab.
            </p>
          </div>
        </FormField>

        <div class="grid grid-cols-2 gap-4">
          <FormField label="Severity">
            <Segmented :model-value="form.severity" @update:model-value="setSeverity">
              <SegmentedItem v-for="s in SEVERITIES" :key="s.value" :value="s.value">{{ s.label }}</SegmentedItem>
            </Segmented>
          </FormField>
          <FormField label="For" hint="How long the condition must hold before the rule triggers.">
            <SelectMenu v-model="form.for_secs" :options="FOR_OPTIONS" content-class="w-32" aria-label="For" />
          </FormField>
        </div>
      </form>

      <DialogFooter class="flex w-full flex-row items-center justify-between gap-2 sm:justify-between">
        <Button type="button" variant="ghost" @click="emit('update:open', false)">Cancel</Button>
        <div class="flex items-center gap-2">
          <Button
            type="button"
            variant="ghost"
            :disabled="(!isEdit && !conditionValid) || testMut.isPending.value"
            @click="testNow"
          >
            Test now
          </Button>
          <Button type="button" :disabled="!canSubmit || pending" @click="submit">
            {{ pending ? 'Saving…' : isEdit ? 'Save changes' : 'Create alert' }}
          </Button>
        </div>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
