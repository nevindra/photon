<script setup lang="ts">
// Quick-setup template picker: pick a target (Service/App/Host/Global) → list of templates for it →
// Apply (POST straight from templateToRuleInput) or Customize (emit a seed → AlertsView opens
// AlertRuleDialog pre-seeded). Frontend-only; see docs/superpowers/specs/2026-07-18-alert-rule-templates-design.md.
import { computed, ref, watch } from 'vue'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { SelectMenu } from '@/components/ui/select-menu'
import { FormField } from '@/components/ui/form-field'
import TemplateRow from './TemplateRow.vue'
import { startNs, endNs } from '@/lib/core/context'
import { useServices } from '@/lib/logs/logsQueries'
import { useRumApps } from '@/lib/rum/rumQueries'
import { useInfraHosts } from '@/lib/infra/infraQueries'
import { useChannels, useCreateRule } from '@/lib/alertsQueries'
import {
  templatesForTarget,
  templateToRuleInput,
  type TemplateTarget,
  type AlertTemplate,
} from '@/lib/alertTemplates'
import type { AlertRuleInput } from '@/lib/core/api'

const props = defineProps<{ open: boolean }>()
const emit = defineEmits<{ 'update:open': [boolean]; customize: [seed: AlertRuleInput] }>()

const TARGETS: { value: TemplateTarget; label: string }[] = [
  { value: 'service', label: 'Service' },
  { value: 'app', label: 'RUM app' },
  { value: 'host', label: 'Host' },
  { value: 'global', label: 'Global' },
]

const target = ref<TemplateTarget>('service')
const selected = ref('') // chosen service/app/host name (unused for global)
const channelIds = ref<string[]>([])

function onPickTarget(v: unknown) {
  if (typeof v === 'string' && v && v !== target.value) {
    target.value = v as TemplateTarget
    selected.value = ''
  }
}
// Reset the transient picks each time the dialog reopens.
watch(() => props.open, (o) => { if (o) { target.value = 'service'; selected.value = ''; channelIds.value = [] } })

// --- target selector options ---
const servicesQuery = useServices()
const rumAppsQuery = useRumApps()
const hostsQuery = useInfraHosts(startNs, endNs)
const targetOptions = computed<{ value: string; label: string }[]>(() => {
  const names =
    target.value === 'service'
      ? servicesQuery.data.value ?? []
      : target.value === 'app'
        ? (rumAppsQuery.data.value?.apps ?? []).map((a) => a.name)
        : target.value === 'host'
          ? (hostsQuery.data.value?.hosts ?? []).map((h) => h.host)
          : []
  return names.map((n) => ({ value: n, label: n }))
})
const needsTarget = computed(() => target.value !== 'global')
const rowsDisabled = computed(() => needsTarget.value && !selected.value)

const templates = computed<AlertTemplate[]>(() => templatesForTarget(target.value))

const channelsQuery = useChannels()
const channels = computed(() => channelsQuery.data.value ?? [])
function toggleChannel(id: string) {
  const i = channelIds.value.indexOf(id)
  if (i === -1) channelIds.value.push(id)
  else channelIds.value.splice(i, 1)
}

const createMut = useCreateRule()

function apply(t: AlertTemplate) {
  const input = templateToRuleInput(t, selected.value, [...channelIds.value])
  createMut.mutate(input, {
    onSuccess: (res) => {
      if (res && res.ok === false) return // useCreateRule already toasts the error
      emit('update:open', false)
    },
  })
}
function customize(t: AlertTemplate) {
  emit('customize', templateToRuleInput(t, selected.value, [...channelIds.value]))
  emit('update:open', false)
}
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-h-[85vh] max-w-2xl overflow-y-auto">
      <DialogHeader>
        <DialogTitle>Browse templates</DialogTitle>
        <DialogDescription>Pick a target, then apply a ready-made alert — customize only if you need to.</DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-5">
        <Segmented :model-value="target" @update:model-value="onPickTarget">
          <SegmentedItem v-for="t in TARGETS" :key="t.value" :value="t.value">{{ t.label }}</SegmentedItem>
        </Segmented>

        <FormField v-if="needsTarget" :label="TARGETS.find((t) => t.value === target)!.label">
          <SelectMenu
            v-if="targetOptions.length"
            v-model="selected"
            :options="targetOptions"
            content-class="w-56"
            :aria-label="`${target} to alert on`"
          />
          <p v-else class="text-xs text-muted-foreground">
            No {{ target }}s discovered yet — send some data first.
          </p>
        </FormField>

        <FormField label="Notify" hint="Channels attached when you Apply. Optional.">
          <div class="flex flex-wrap gap-2">
            <button
              v-for="c in channels"
              :key="c.id"
              type="button"
              :aria-pressed="channelIds.includes(c.id)"
              class="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors"
              :class="channelIds.includes(c.id) ? 'border-brand/40 bg-brand/10 text-brand' : 'border-border bg-muted text-muted-foreground hover:text-foreground'"
              @click="toggleChannel(c.id)"
            >
              {{ c.name }}
            </button>
            <p v-if="!channels.length" class="text-xs text-muted-foreground">
              No channels yet; the rule will be created without notifications — add one on the Channels tab.
            </p>
          </div>
        </FormField>

        <div class="flex flex-col gap-2">
          <p v-if="rowsDisabled" class="text-xs text-muted-foreground">
            Pick a {{ target }} above to apply a template.
          </p>
          <TemplateRow
            v-for="t in templates"
            :key="t.id"
            :template="t"
            :disabled="rowsDisabled"
            @apply="apply(t)"
            @customize="customize(t)"
          />
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
