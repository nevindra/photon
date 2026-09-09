<script setup lang="ts">
// Volumes on the host. The one action is remove, and only for a volume nothing mounts — a
// stopped container's data is still that container's, so `used_by` (not "running") gates it.
import { Trash2 } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { Badge } from '@/components/ui/badge'
import { EmptyState } from '@/components/ui/empty-state'
import { formatBytes } from '@/lib/core/format'
import { ageMs } from '@/lib/deploy/audit'
import type { VolumeInfo } from '@/lib/core/api'

defineProps<{ volumes: VolumeInfo[]; blocked?: string | null }>()
defineEmits<{ remove: [volume: VolumeInfo] }>()
</script>

<template>
  <section class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="volumes-table">
    <Table container-class="overflow-x-auto" class="text-sm">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead>Volume</TableHead>
          <TableHead class="w-24">Driver</TableHead>
          <TableHead class="w-28 text-right">Size</TableHead>
          <TableHead>Used by</TableHead>
          <TableHead class="w-28 text-right">Created</TableHead>
          <TableHead class="w-20 text-right">Actions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="v in volumes" :key="v.name" data-testid="volume-row" class="border-border/60">
          <TableCell class="py-2">
            <div class="flex flex-wrap items-center gap-1.5">
              <span class="font-mono text-xs text-foreground">{{ v.name }}</span>
              <Badge v-if="!v.used_by.length" variant="outline">unused</Badge>
            </div>
            <div class="truncate font-mono text-[11px] text-muted-foreground/70" :title="v.mountpoint">{{ v.mountpoint }}</div>
          </TableCell>
          <TableCell class="py-2 text-xs text-muted-foreground">{{ v.driver }}</TableCell>
          <TableCell class="py-2 text-right tabular-nums">{{ v.size_bytes == null ? '—' : formatBytes(v.size_bytes) }}</TableCell>
          <TableCell class="py-2">
            <div class="flex flex-wrap gap-1">
              <Badge v-for="c in v.used_by" :key="c" variant="outline" class="font-mono">{{ c }}</Badge>
              <span v-if="!v.used_by.length" class="text-xs text-muted-foreground/60">nothing — a prune would remove it</span>
            </div>
          </TableCell>
          <TableCell class="py-2 text-right tabular-nums text-muted-foreground">{{ ageMs(v.created_ms) }}</TableCell>
          <TableCell class="py-1.5 text-right">
            <Tooltip><TooltipTrigger as-child><span class="inline-flex"><Button variant="ghost" size="icon" class="size-7 text-muted-foreground hover:text-sev-error" :disabled="!!blocked || v.used_by.length > 0" :aria-label="`Remove ${v.name}`" data-volume-action="remove" @click="$emit('remove', v)"><Trash2 class="size-3.5" /></Button></span></TooltipTrigger><TooltipContent>{{ blocked ?? (v.used_by.length ? `In use by ${v.used_by.join(', ')}` : 'Remove the volume') }}</TooltipContent></Tooltip>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
    <EmptyState v-if="!volumes.length" title="No volumes on this host" class="h-auto py-10" />
  </section>
</template>
