<script setup lang="ts">
// `docker compose up` knobs (R4.3): pull policy, force recreate, and recreate volumes — the last
// behind its own data-loss line, because it is the one that can delete a customer's database.
//
// Two groups, because the three knobs answer two different questions. "Images" is one pick out
// of three, drawn as option cards that each say what they do — a segmented control read as
// three bare words, and "never" is not a word an operator should have to guess the meaning of
// before shipping. "Containers" is two independent switches on a settings list, label and
// consequence left, control right, the same row shape the Target step uses.
import { RadioGroupRoot, RadioGroupItem } from 'reka-ui'
import { Check, TriangleAlert } from 'lucide-vue-next'
import { Switch } from '@/components/ui/switch'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/core/utils'
import type { DeployOptions } from '@/lib/core/api'

const props = defineProps<{ modelValue: DeployOptions }>()
const emit = defineEmits<{ 'update:modelValue': [DeployOptions] }>()
function set<K extends keyof DeployOptions>(k: K, v: DeployOptions[K]): void {
  emit('update:modelValue', { ...props.modelValue, [k]: v })
}

const PULL: { value: DeployOptions['pull']; label: string; detail: string; note?: string }[] = [
  { value: 'missing', label: 'Missing', detail: 'Pull only images the host does not have yet. Cached tags are reused as they are.', note: 'Default' },
  { value: 'always', label: 'Always', detail: 'Re-pull every image, even ones already on the host, so a moved tag like latest is picked up.' },
  { value: 'never', label: 'Never', detail: 'Never contact a registry. The deploy fails if any image is not already on the host.' },
]
</script>

<template>
  <div class="flex flex-col gap-5" data-testid="deploy-options">
    <!-- Images -->
    <section class="flex flex-col gap-2.5">
      <div class="flex flex-col gap-0.5">
        <p id="pull-policy-label" class="text-sm font-medium leading-snug text-foreground">Images</p>
        <p class="text-xs leading-relaxed text-muted-foreground">Which images the host fetches before it starts the stack.</p>
      </div>
      <RadioGroupRoot
        :model-value="modelValue.pull"
        aria-labelledby="pull-policy-label"
        class="grid gap-2 sm:grid-cols-3"
        data-testid="pull-policy"
        @update:model-value="set('pull', $event as DeployOptions['pull'])"
      >
        <RadioGroupItem
          v-for="p in PULL"
          :key="p.value"
          :value="p.value"
          :data-testid="`pull-${p.value}`"
          :class="cn(
            'group flex min-w-0 flex-col gap-1.5 rounded-lg border border-border bg-card p-3 text-left transition-[border-color,background-color,box-shadow] duration-150 ease-out',
            'hover:border-foreground/25 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background',
            'data-[state=checked]:border-brand data-[state=checked]:bg-brand-soft/50 data-[state=checked]:shadow-[inset_0_0_0_1px_hsl(var(--brand))]',
          )"
        >
          <span class="flex items-center gap-2">
            <span class="flex size-4 shrink-0 items-center justify-center rounded-full border border-input bg-surface-1 transition-colors group-data-[state=checked]:border-brand group-data-[state=checked]:bg-brand">
              <Check class="size-2.5 text-brand-foreground opacity-0 transition-opacity group-data-[state=checked]:opacity-100" stroke-width="3" />
            </span>
            <span class="text-sm font-medium text-foreground">{{ p.label }}</span>
            <Badge v-if="p.note" variant="outline" class="ml-auto h-4 px-1.5 text-[10px] font-normal text-muted-foreground">{{ p.note }}</Badge>
          </span>
          <span class="text-xs leading-relaxed text-muted-foreground">{{ p.detail }}</span>
        </RadioGroupItem>
      </RadioGroupRoot>
    </section>

    <!-- Containers -->
    <section class="flex flex-col gap-2">
      <div class="flex flex-col gap-0.5">
        <p class="text-sm font-medium leading-snug text-foreground">Containers</p>
        <p class="text-xs leading-relaxed text-muted-foreground">Both are off for an ordinary deploy. Compose already replaces what changed.</p>
      </div>
      <div class="divide-y divide-border overflow-hidden rounded-lg border border-border bg-card">
        <label class="flex cursor-pointer items-center justify-between gap-4 px-3.5 py-3 transition-colors hover:bg-muted/40" for="force-recreate">
          <span class="flex min-w-0 flex-col gap-0.5">
            <span class="text-sm font-medium text-foreground">Force recreate</span>
            <span class="text-xs leading-relaxed text-muted-foreground">Recreate every container, even one whose image and config did not change.</span>
          </span>
          <Switch id="force-recreate" :model-value="modelValue.force_recreate" data-testid="force-recreate" @update:model-value="set('force_recreate', $event)" />
        </label>
        <label
          :class="cn('flex cursor-pointer items-center justify-between gap-4 px-3.5 py-3 transition-colors', modelValue.recreate_volumes ? 'bg-sev-warn-soft/60' : 'hover:bg-muted/40')"
          for="recreate-volumes"
          :data-state="modelValue.recreate_volumes ? 'on' : 'off'"
        >
          <span class="flex min-w-0 flex-col gap-0.5">
            <span class="flex items-center gap-1.5 text-sm font-medium text-foreground">
              Recreate anonymous volumes
              <TriangleAlert v-if="modelValue.recreate_volumes" class="size-3.5 text-sev-warn" />
            </span>
            <span :class="cn('text-xs leading-relaxed', modelValue.recreate_volumes ? 'text-sev-warn' : 'text-muted-foreground')">
              <template v-if="modelValue.recreate_volumes">Data in anonymous volumes will be lost on this deploy. Named volumes are never touched.</template>
              <template v-else>Drop and recreate anonymous volumes. Their data is lost. Named volumes are never touched.</template>
            </span>
          </span>
          <Switch id="recreate-volumes" :model-value="modelValue.recreate_volumes" data-testid="recreate-volumes" @update:model-value="set('recreate_volumes', $event)" />
        </label>
      </div>
    </section>
  </div>
</template>
