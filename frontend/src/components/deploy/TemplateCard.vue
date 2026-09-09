<script setup lang="ts">
// One template in the catalog. The card is the thing an operator scans: icon, name, one line of
// what it is, categories, the latest version and where it already runs. Click opens the detail;
// Deploy starts the wizard straight from the grid — the common case should not need a detour.
import { RouterLink } from 'vue-router'
import { Rocket } from 'lucide-vue-next'
import { Card } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import TemplateIcon from './TemplateIcon.vue'
import GatedButton from './GatedButton.vue'
import { ageMs } from '@/lib/deploy/audit'
import type { Template } from '@/lib/core/api'

defineProps<{ template: Template }>()
defineEmits<{ open: [t: Template]; deploy: [t: Template] }>()
</script>

<template>
  <!-- The title is the link (keyboard, screen reader); the card's click is a pointer convenience. -->
  <Card interactive class="flex flex-col gap-3 p-4" data-testid="template-card" :data-template="template.id" @click="$emit('open', template)">
    <div class="flex items-start gap-3">
      <TemplateIcon :icon="template.icon" :name="template.name" />
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <RouterLink :to="`/deploy/templates/${encodeURIComponent(template.id)}`" class="truncate font-medium text-foreground hover:underline" @click.stop>{{ template.name }}</RouterLink>
          <Badge v-if="template.source !== 'local'" variant="outline" class="shrink-0">{{ template.source === 'portainer' ? 'portainer' : 'registry' }}</Badge>
        </div>
        <p class="mt-0.5 line-clamp-2 text-xs text-muted-foreground">{{ template.description || 'No description.' }}</p>
      </div>
    </div>
    <div class="flex flex-wrap gap-1">
      <Badge v-for="c in template.categories" :key="c" variant="secondary" class="font-normal">{{ c }}</Badge>
    </div>
    <div class="mt-auto flex items-center gap-2 border-t border-border pt-3 text-xs text-muted-foreground">
      <span class="font-mono text-foreground">{{ template.latest_version }}</span>
      <span>· {{ template.versions.length }} {{ template.versions.length === 1 ? 'version' : 'versions' }}</span>
      <span v-if="template.stacks">· on {{ template.stacks }} {{ template.stacks === 1 ? 'host' : 'hosts' }}</span>
      <span class="ml-auto">{{ ageMs(template.updated_ms) }}</span>
      <GatedButton perm="deploy" size="sm" variant="outline" class="h-7 px-2 text-xs" :data-deploy="template.id" @click.stop="$emit('deploy', template)"><Rocket class="size-3.5" /> Deploy</GatedButton>
    </div>
  </Card>
</template>
