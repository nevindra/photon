<script setup lang="ts">
// "Which version is customer X on" — fleet-wide, for one template: every host running it, the
// version, the generation gap, its status, grouped by Site. The action falls out of the answer:
// pick the hosts behind the latest version and roll it out to them in one click.
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { Rocket, Server } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { EmptyState } from '@/components/ui/empty-state'
import StackStatusPill from './StackStatusPill.vue'
import GatedButton from './GatedButton.vue'
import NewRolloutDialog from './NewRolloutDialog.vue'
import { semverCompare } from '@/lib/deploy/templates'
import { generationLabel } from '@/lib/deploy/stacks'
import { ageMs } from '@/lib/deploy/audit'
import type { Stack, Template } from '@/lib/core/api'

const props = defineProps<{ template: Template; stacks: Stack[] }>()
const latest = computed(() => props.template.latest_version)
const rows = computed(() => [...props.stacks].sort((a, b) => (a.site ?? '').localeCompare(b.site ?? '') || a.host.localeCompare(b.host)))
const behind = computed(() => rows.value.filter((s) => s.template_version && semverCompare(latest.value, s.template_version) > 0 && s.status !== 'gone'))
const versions = computed(() => {
  const m = new Map<string, number>()
  for (const s of rows.value) m.set(s.template_version ?? '?', (m.get(s.template_version ?? '?') ?? 0) + 1)
  return Array.from(m.entries()).sort((a, b) => semverCompare(b[0], a[0]))
})
const rolloutOpen = ref(false)
</script>

<template>
  <section class="flex flex-col gap-3" data-testid="version-matrix">
    <div class="flex flex-wrap items-center gap-2">
      <div class="flex flex-wrap gap-1.5">
        <Badge v-for="[v, n] in versions" :key="v" :variant="v === latest ? 'brand' : 'outline'" class="font-mono" :data-version-count="v">{{ v }} <span class="ml-1 font-sans tabular-nums opacity-70">× {{ n }}</span></Badge>
      </div>
      <GatedButton v-if="behind.length" perm="deploy" size="sm" variant="brand" class="ml-auto" data-testid="rollout-behind" @click="rolloutOpen = true">
        <Rocket class="size-4" /> Roll out {{ latest }} to the {{ behind.length }} host{{ behind.length === 1 ? '' : 's' }} behind
      </GatedButton>
      <span v-else-if="rows.length" class="ml-auto text-xs text-success">Every host is on {{ latest }}.</span>
    </div>
    <div class="overflow-hidden rounded-xl border border-border bg-card shadow-1">
      <table class="w-full text-sm">
        <thead><tr class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground"><th class="px-4 py-2 text-left">Host</th><th class="px-2 py-2 text-left">Site</th><th class="px-2 py-2 text-left">Version</th><th class="px-2 py-2 text-left">Generation</th><th class="px-2 py-2 text-left">Status</th><th class="px-4 py-2 text-right">Applied</th></tr></thead>
        <tbody>
          <tr v-for="s in rows" :key="s.id" class="border-t border-border/60" :data-matrix-host="s.host">
            <td class="whitespace-nowrap px-4 py-2"><RouterLink :to="`/deploy/stacks/${encodeURIComponent(s.id)}`" class="inline-flex items-center gap-1.5 font-mono text-xs text-foreground hover:underline"><Server class="size-3 text-muted-foreground" />{{ s.host }}</RouterLink></td>
            <td class="px-2 py-2 text-xs text-muted-foreground">{{ s.site ?? 'this Photon' }}</td>
            <td class="px-2 py-2"><span class="font-mono text-xs" :class="s.template_version === latest ? 'text-foreground' : 'text-sev-warn'">{{ s.template_version ?? '?' }}</span><span v-if="s.template_version !== latest" class="ml-1.5 text-[11px] text-sev-warn">behind</span></td>
            <td class="px-2 py-2 font-mono text-xs tabular-nums text-muted-foreground">{{ generationLabel(s) }}</td>
            <td class="px-2 py-2"><StackStatusPill :status="s.status" /></td>
            <td class="whitespace-nowrap px-4 py-2 text-right text-xs tabular-nums text-muted-foreground">{{ s.last_applied_ms ? ageMs(s.last_applied_ms) : 'never' }}</td>
          </tr>
        </tbody>
      </table>
      <EmptyState v-if="!rows.length" title="Not deployed anywhere yet" description="Deploy it to a host, or roll it out to a Site." class="h-auto py-10" />
    </div>
    <NewRolloutDialog :open="rolloutOpen" :template-id="template.id" :prefill-hosts="behind.map((s) => s.host)" @update:open="rolloutOpen = $event" />
  </section>
</template>
