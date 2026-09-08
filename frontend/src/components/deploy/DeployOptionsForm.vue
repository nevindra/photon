<script setup lang="ts">
// `docker compose up` knobs (R4.3): pull policy, force recreate, and recreate volumes — the last
// behind its own data-loss line, because it is the one that can delete a customer's database.
import { TriangleAlert } from 'lucide-vue-next'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Switch } from '@/components/ui/switch'
import { FormField } from '@/components/ui/form-field'
import type { DeployOptions } from '@/lib/core/api'

const props = defineProps<{ modelValue: DeployOptions; compact?: boolean }>()
const emit = defineEmits<{ 'update:modelValue': [DeployOptions] }>()
function set<K extends keyof DeployOptions>(k: K, v: DeployOptions[K]): void {
  emit('update:modelValue', { ...props.modelValue, [k]: v })
}
</script>

<template>
  <div class="grid gap-3" :class="compact ? '' : 'sm:grid-cols-2'" data-testid="deploy-options">
    <FormField label="Pull policy" for="pull-policy" hint="missing pulls only images the host lacks; always re-pulls every tag; never fails if an image is absent.">
      <Segmented id="pull-policy" :model-value="modelValue.pull" data-testid="pull-policy" @update:model-value="set('pull', $event as DeployOptions['pull'])">
        <SegmentedItem value="missing">Missing</SegmentedItem>
        <SegmentedItem value="always">Always</SegmentedItem>
        <SegmentedItem value="never">Never</SegmentedItem>
      </Segmented>
    </FormField>
    <div class="flex flex-col gap-3">
      <label class="flex items-start justify-between gap-3" for="force-recreate">
        <span class="text-sm"><span class="font-medium text-foreground">Force recreate</span><span class="block text-xs text-muted-foreground">Recreate containers even when nothing changed.</span></span>
        <Switch id="force-recreate" :model-value="modelValue.force_recreate" data-testid="force-recreate" @update:model-value="set('force_recreate', $event)" />
      </label>
      <label class="flex items-start justify-between gap-3" for="recreate-volumes">
        <span class="text-sm"><span class="font-medium text-foreground">Recreate anonymous volumes</span><span class="block text-xs" :class="modelValue.recreate_volumes ? 'text-sev-warn' : 'text-muted-foreground'"><TriangleAlert v-if="modelValue.recreate_volumes" class="mr-1 inline size-3" />Data in anonymous volumes is lost. Named volumes are never touched here.</span></span>
        <Switch id="recreate-volumes" :model-value="modelValue.recreate_volumes" data-testid="recreate-volumes" @update:model-value="set('recreate_volumes', $event)" />
      </label>
    </div>
  </div>
</template>
