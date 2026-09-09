<script setup lang="ts">
// A container's actions: one primary button (Restart when running, Start when stopped) and a
// "more" menu with the rest, in the order `availableActions` gives. Disabled — never hidden —
// when the role, the agent's capabilities, or an in-flight action say no, with the reason on
// hover. Emits the kind; the panel owns the confirm dialog and the request.
import { computed } from 'vue'
import { MoreHorizontal, Play, RotateCw, Square, Zap, Download, RefreshCw, Trash2, Loader2 } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuLabel } from '@/components/ui/dropdown-menu'
import { ACTION_META, availableActions, primaryAction } from '@/lib/deploy/actions'
import type { ActionKind, Container } from '@/lib/core/api'

const props = defineProps<{
  container: Container
  /** null = allowed; otherwise the sentence explaining why not. */
  blocked: string | null
  /** An action already queued/running for this container. */
  busy?: ActionKind | null
  size?: 'sm' | 'default'
}>()
const emit = defineEmits<{ act: [kind: ActionKind] }>()

const ICON: Record<ActionKind, unknown> = { start: Play, restart: RotateCw, stop: Square, kill: Zap, pull: Download, recreate: RefreshCw, remove: Trash2, prune: Trash2, image_pull: Download, image_remove: Trash2, volume_remove: Trash2 }
const primary = computed(() => primaryAction(props.container))
const rest = computed(() => availableActions(props.container).filter((k) => k !== primary.value))
const disabled = computed(() => !!props.blocked || !!props.busy)
const hint = computed(() => props.blocked ?? (props.busy ? `${ACTION_META[props.busy].progressive} — wait for it to finish` : null))
</script>

<template>
  <div class="flex items-center gap-1" data-testid="container-actions" @click.stop @keydown.enter.stop>
    <Tooltip v-if="primary">
      <TooltipTrigger as-child>
        <span class="inline-flex">
          <Button variant="outline" :size="size === 'default' ? 'sm' : 'sm'" class="h-7 px-2 text-xs" :disabled="disabled" :data-action="primary" @click="emit('act', primary!)">
            <Loader2 v-if="busy" class="size-3.5 animate-spin" />
            <component :is="ICON[primary]" v-else class="size-3.5" />
            {{ busy ? ACTION_META[busy].progressive : ACTION_META[primary].label }}
          </Button>
        </span>
      </TooltipTrigger>
      <TooltipContent v-if="hint">{{ hint }}</TooltipContent>
    </Tooltip>
    <DropdownMenu v-if="rest.length">
      <DropdownMenuTrigger as-child>
        <Button variant="ghost" size="icon" class="size-7" :disabled="disabled" aria-label="More actions" data-testid="container-more">
          <MoreHorizontal class="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" class="w-48">
        <DropdownMenuLabel class="font-mono text-[11px] font-normal text-muted-foreground">{{ container.name }}</DropdownMenuLabel>
        <template v-for="k in rest" :key="k">
          <DropdownMenuSeparator v-if="ACTION_META[k].destructive && k === 'remove'" />
          <DropdownMenuItem :data-action="k" :class="ACTION_META[k].destructive && 'text-sev-error focus:text-sev-error'" @select="emit('act', k)">
            <component :is="ICON[k]" class="size-4" /> {{ ACTION_META[k].label }}
          </DropdownMenuItem>
        </template>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
