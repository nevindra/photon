<script setup lang="ts">
// Images on the host, largest first. An image nothing uses is what prune reclaims, so "unused"
// and "dangling" are the states worth a chip.
import { Download, Trash2 } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { Badge } from '@/components/ui/badge'
import { EmptyState } from '@/components/ui/empty-state'
import { shortDigest } from '@/lib/deploy/containers'
import { formatBytes } from '@/lib/core/format'
import { ageMs } from '@/lib/deploy/audit'
import type { ImageInfo } from '@/lib/core/api'

defineProps<{ images: ImageInfo[]; blocked?: string | null }>()
defineEmits<{ act: [image: ImageInfo, kind: 'image_pull' | 'image_remove'] }>()
</script>

<template>
  <section class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="images-table">
    <Table container-class="overflow-x-auto" class="text-sm">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead>Image</TableHead>
          <TableHead class="w-32">ID</TableHead>
          <TableHead class="w-28 text-right">Size</TableHead>
          <TableHead class="w-28 text-right">Created</TableHead>
          <TableHead class="w-28 text-right">Used by</TableHead>
          <TableHead class="w-24 text-right">Actions</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="img in images" :key="img.id" data-testid="image-row" class="border-border/60">
          <TableCell class="py-2">
            <div class="flex flex-wrap items-center gap-1.5">
              <span class="font-mono text-xs text-foreground">{{ img.repo_tags[0] ?? '<none>' }}</span>
              <Badge v-if="img.dangling" variant="warning">dangling</Badge>
              <Badge v-else-if="!img.used_by" variant="outline">unused</Badge>
              <span v-if="img.repo_tags.length > 1" class="text-[11px] text-muted-foreground">+{{ img.repo_tags.length - 1 }} tags</span>
            </div>
          </TableCell>
          <TableCell class="py-2 font-mono text-xs text-muted-foreground">{{ shortDigest(img.id) }}</TableCell>
          <TableCell class="py-2 text-right tabular-nums">{{ formatBytes(img.size_bytes) }}</TableCell>
          <TableCell class="py-2 text-right tabular-nums text-muted-foreground">{{ ageMs(img.created_ms) }}</TableCell>
          <TableCell class="py-2 text-right tabular-nums" :class="img.used_by ? 'text-foreground' : 'text-muted-foreground'">{{ img.used_by }} {{ img.used_by === 1 ? 'container' : 'containers' }}</TableCell>
          <TableCell class="py-1.5 text-right">
            <div class="inline-flex items-center gap-0.5">
              <Tooltip><TooltipTrigger as-child><span class="inline-flex"><Button v-if="img.repo_tags.length" variant="ghost" size="icon" class="size-7" :disabled="!!blocked" :aria-label="`Pull ${img.repo_tags[0]}`" data-image-action="pull" @click="$emit('act', img, 'image_pull')"><Download class="size-3.5" /></Button></span></TooltipTrigger><TooltipContent>{{ blocked ?? 'Pull the tag again' }}</TooltipContent></Tooltip>
              <Tooltip><TooltipTrigger as-child><span class="inline-flex"><Button variant="ghost" size="icon" class="size-7 text-muted-foreground hover:text-sev-error" :disabled="!!blocked || img.used_by > 0" :aria-label="`Remove ${img.repo_tags[0] ?? img.id}`" data-image-action="remove" @click="$emit('act', img, 'image_remove')"><Trash2 class="size-3.5" /></Button></span></TooltipTrigger><TooltipContent>{{ blocked ?? (img.used_by ? `In use by ${img.used_by} container${img.used_by === 1 ? '' : 's'}` : 'Remove the image') }}</TooltipContent></Tooltip>
            </div>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
    <EmptyState v-if="!images.length" title="No images on this host" class="h-auto py-10" />
  </section>
</template>
