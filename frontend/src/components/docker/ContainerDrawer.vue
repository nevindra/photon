<script setup lang="ts">
// The per-container drawer: one Sheet with Logs / Stats / Details as in-page Reka Tabs (panels,
// not routes). Opens from a row in the inventory; the URL carries `?c=<name>` so a drawer can be
// linked and survives a reload. Lifecycle actions arrive in the header in the next phase.
import { computed, ref } from 'vue'
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetDescription } from '@/components/ui/sheet'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import { StatusDot } from '@/components/ui/status-dot'
import { StatusPill } from '@/components/ui/status-pill'
import { Badge } from '@/components/ui/badge'
import ContainerLogView from './ContainerLogView.vue'
import ContainerStatsView from './ContainerStatsView.vue'
import ContainerDetails from './ContainerDetails.vue'
import ContainerShell from './ContainerShell.vue'
import { containerTone, HEALTH_META, STATE_META, isRunning } from '@/lib/deploy/containers'
import type { Container } from '@/lib/core/api'

const props = defineProps<{ open: boolean; host: string; container: Container | null; execBlocked?: string | null }>()
const emit = defineEmits<{ 'update:open': [boolean] }>()

const tab = ref<'logs' | 'stats' | 'details' | 'shell'>('logs')
const c = computed(() => props.container)
</script>

<template>
  <Sheet :open="open" @update:open="emit('update:open', $event)">
    <SheetContent v-if="c" side="right" class="flex w-[46rem] max-w-[95vw] flex-col gap-0 p-0 sm:max-w-[46rem]" data-testid="container-drawer">
      <SheetHeader class="shrink-0 border-b border-border px-5 py-4 text-left">
        <div class="flex items-start gap-3">
          <StatusDot :tone="containerTone(c)" size="lg" class="mt-1.5" />
          <div class="min-w-0 flex-1">
            <SheetTitle class="flex flex-wrap items-center gap-2 text-base">
              <span class="font-mono">{{ c.name }}</span>
              <StatusPill :tone="STATE_META[c.state].tone">{{ STATE_META[c.state].label }}</StatusPill>
              <StatusPill v-if="HEALTH_META[c.health]" :tone="HEALTH_META[c.health]!.tone">{{ HEALTH_META[c.health]!.label }}</StatusPill>
            </SheetTitle>
            <SheetDescription class="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs">
              <span class="font-mono">{{ c.image }}</span>
              <span>·</span>
              <span>{{ c.status_text }}</span>
              <Badge v-if="c.compose_project" variant="outline" class="font-mono">{{ c.compose_project }} / {{ c.compose_service }}</Badge>
            </SheetDescription>
          </div>
        </div>
        <div v-if="$slots.actions" class="mt-3 flex items-center gap-2"><slot name="actions" /></div>
      </SheetHeader>
      <Tabs v-model="tab" class="flex min-h-0 flex-1 flex-col">
        <TabsList class="mx-5 mt-3 w-fit shrink-0">
          <TabsTrigger value="logs" data-testid="tab-logs">Logs</TabsTrigger>
          <TabsTrigger value="stats" data-testid="tab-stats">Stats</TabsTrigger>
          <TabsTrigger value="details" data-testid="tab-details">Details</TabsTrigger>
          <TabsTrigger value="shell" data-testid="tab-shell">Shell</TabsTrigger>
        </TabsList>
        <TabsContent value="logs" class="mt-3 min-h-0 flex-1 border-t border-border data-[state=inactive]:hidden">
          <ContainerLogView :key="c.id" :host="host" :container-id="c.id" :container-name="c.name" :running="isRunning(c)" />
        </TabsContent>
        <TabsContent value="stats" class="mt-3 min-h-0 flex-1 overflow-y-auto border-t border-border data-[state=inactive]:hidden">
          <ContainerStatsView :key="c.id" :host="host" :container-id="c.id" />
        </TabsContent>
        <TabsContent value="details" class="mt-3 min-h-0 flex-1 overflow-y-auto border-t border-border data-[state=inactive]:hidden">
          <ContainerDetails :container="c" />
        </TabsContent>
        <TabsContent value="shell" class="mt-3 min-h-0 flex-1 border-t border-border data-[state=inactive]:hidden">
          <ContainerShell :key="c.id" :host="host" :container-id="c.id" :container-name="c.name" :blocked="execBlocked ?? (isRunning(c) ? null : 'The container is not running.')" />
        </TabsContent>
      </Tabs>
    </SheetContent>
  </Sheet>
</template>
