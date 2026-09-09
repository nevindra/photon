<script setup lang="ts">
// The glance above the inventory as ONE line: how many are running, whether any is unhealthy,
// how restart-happy the host is, and how much disk Docker holds — with a real bar for the disk
// and Prune beside the slice it would free. Four numbers that are usually "fine" do not earn
// four tiles; they earn a line. Colour only where it says something.
import { computed } from 'vue'
import { Trash2 } from 'lucide-vue-next'
import { StatusDot } from '@/components/ui/status-dot'
import { Meter } from '@/components/ui/meter'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { inventoryKpis } from '@/lib/deploy/containers'
import { formatBytes, formatNumber } from '@/lib/core/format'
import type { HostInventory } from '@/lib/core/api'

const props = defineProps<{
  inventory: HostInventory
  /** Why a prune is unavailable on this host (role / capability), or null. */
  blocked?: string | null
}>()
const emit = defineEmits<{ prune: [] }>()
const k = computed(() => inventoryKpis(props.inventory))
const disk = computed(() => props.inventory.facts?.disk ?? null)
const dockerBytes = computed(() => (disk.value ? disk.value.images_bytes + disk.value.volumes_bytes + disk.value.build_cache_bytes : null))
// The bar is Docker's share of the filesystem when the agent reports one, else how much of what
// Docker holds a prune would give back — either way, "how full is this" at a glance.
const ratio = computed(() => {
  const d = disk.value
  if (!d) return null
  if (d.total_bytes && d.used_bytes != null) return d.used_bytes / d.total_bytes
  return dockerBytes.value ? d.reclaimable_bytes / dockerBytes.value : null
})
const diskTone = computed(() => (disk.value?.used_bytes && disk.value.total_bytes && disk.value.used_bytes / disk.value.total_bytes >= 0.9 ? 'error' : 'info'))
const runningTone = computed(() => (k.value.unhealthy ? 'error' : k.value.total && !k.value.running ? 'neutral' : 'success'))
</script>

<template>
  <div class="flex flex-wrap items-stretch rounded-xl border border-border bg-card px-1.5 text-sm shadow-1" data-testid="container-kpis">
    <div class="flex items-center gap-2.5 px-3.5 py-2.5" data-slot="running">
      <StatusDot :tone="runningTone" size="lg" />
      <span class="text-[15px] font-semibold tabular-nums text-foreground">{{ formatNumber(k.running) }} / {{ formatNumber(k.total) }}</span>
      <span class="text-muted-foreground">running<template v-if="k.projects"> · {{ k.projects }} compose {{ k.projects === 1 ? 'project' : 'projects' }}</template></span>
    </div>
    <div class="flex items-center gap-2 border-l border-border px-3.5 py-2.5" data-slot="unhealthy">
      <span class="text-[15px] font-semibold tabular-nums" :class="k.unhealthy ? 'text-sev-error' : 'text-foreground'">{{ formatNumber(k.unhealthy) }}</span>
      <span class="text-muted-foreground" :title="k.unhealthy ? 'failing a health check or crash-looping' : 'every health check passing'">unhealthy</span>
    </div>
    <div class="flex items-center gap-2 border-l border-border px-3.5 py-2.5" data-slot="restarts">
      <span class="text-[15px] font-semibold tabular-nums" :class="k.restarts >= 5 ? 'text-sev-warn' : 'text-foreground'">{{ formatNumber(k.restarts) }}</span>
      <span class="text-muted-foreground" title="since each container was created">restarts</span>
    </div>
    <div class="ml-auto flex flex-wrap items-center gap-3 border-l border-border py-1.5 pl-3.5 pr-1" data-slot="disk">
      <span class="text-xs text-muted-foreground">Docker disk</span>
      <span class="text-sm font-semibold tabular-nums text-foreground">{{ dockerBytes == null ? '—' : formatBytes(dockerBytes) }}</span>
      <Meter v-if="ratio != null" :value="ratio" :tone="diskTone" class="w-28" />
      <span class="text-xs text-muted-foreground">{{ disk?.reclaimable_bytes ? `${formatBytes(disk.reclaimable_bytes)} reclaimable` : dockerBytes == null ? 'not reported by the agent' : 'images, volumes and build cache' }}</span>
      <GatedButton perm="lifecycle" size="sm" variant="outline" class="h-7" :disabled="!!blocked" :title="blocked ?? undefined" data-testid="prune-open" @click="emit('prune')"><Trash2 class="size-3.5" /> Prune…</GatedButton>
    </div>
  </div>
</template>
