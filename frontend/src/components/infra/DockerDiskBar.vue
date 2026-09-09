<script setup lang="ts">
// How a host's disk is spent on Docker: one stacked bar (images / volumes / build cache) over the
// disk's total, with the reclaimable slice called out. Disk is the number that kills on-prem
// boxes, so it gets a picture rather than four numbers in a row.
import { computed } from 'vue'
import { formatBytes } from '@/lib/core/format'
import type { DockerDisk } from '@/lib/core/api'

const props = defineProps<{ disk: DockerDisk }>()

const SEGMENTS = [
  { key: 'images_bytes', label: 'Images', cls: 'bg-brand' },
  { key: 'volumes_bytes', label: 'Volumes', cls: 'bg-brand/55' },
  { key: 'build_cache_bytes', label: 'Build cache', cls: 'bg-brand/30' },
] as const

const dockerBytes = computed(() => props.disk.images_bytes + props.disk.volumes_bytes + props.disk.build_cache_bytes)
// The bar's denominator is the whole disk when known, else Docker's own footprint.
const denom = computed(() => props.disk.total_bytes ?? dockerBytes.value)
const otherBytes = computed(() => Math.max(0, (props.disk.used_bytes ?? dockerBytes.value) - dockerBytes.value))
const pct = (b: number) => (denom.value > 0 ? Math.max(0, Math.min(100, (b / denom.value) * 100)) : 0)
const usedPct = computed(() => (props.disk.used_bytes != null && props.disk.total_bytes ? pct(props.disk.used_bytes) : null))
const tone = computed(() => (usedPct.value == null ? '' : usedPct.value >= 90 ? 'text-sev-error' : usedPct.value >= 80 ? 'text-sev-warn' : 'text-foreground'))
</script>

<template>
  <div class="flex flex-col gap-2" data-testid="docker-disk">
    <div class="flex items-baseline justify-between gap-3 text-xs">
      <span class="text-muted-foreground">Docker uses <span class="font-medium text-foreground">{{ formatBytes(dockerBytes) }}</span><template v-if="disk.total_bytes"> of {{ formatBytes(disk.total_bytes) }}</template></span>
      <span v-if="usedPct != null" :class="['tabular-nums font-medium', tone]">{{ Math.round(usedPct) }}% of disk used</span>
    </div>
    <div class="flex h-2.5 w-full overflow-hidden rounded-full bg-muted" role="img" :aria-label="`Images ${formatBytes(disk.images_bytes)}, volumes ${formatBytes(disk.volumes_bytes)}, build cache ${formatBytes(disk.build_cache_bytes)}`">
      <span v-for="s in SEGMENTS" :key="s.key" :class="s.cls" :style="{ width: pct(disk[s.key]) + '%' }" :title="`${s.label}: ${formatBytes(disk[s.key])}`" />
      <span v-if="otherBytes" class="bg-muted-foreground/30" :style="{ width: pct(otherBytes) + '%' }" title="Everything else on the disk" />
    </div>
    <div class="flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-muted-foreground">
      <span v-for="s in SEGMENTS" :key="s.key" class="inline-flex items-center gap-1.5">
        <span :class="['size-2 rounded-sm', s.cls]" /> {{ s.label }} <span class="tabular-nums text-foreground">{{ formatBytes(disk[s.key]) }}</span>
      </span>
      <span v-if="otherBytes" class="inline-flex items-center gap-1.5"><span class="size-2 rounded-sm bg-muted-foreground/30" /> Other <span class="tabular-nums text-foreground">{{ formatBytes(otherBytes) }}</span></span>
      <span v-if="disk.reclaimable_bytes" class="ml-auto inline-flex items-center gap-1 text-success" data-testid="docker-reclaimable">
        {{ formatBytes(disk.reclaimable_bytes) }} reclaimable by prune
      </span>
    </div>
  </div>
</template>
