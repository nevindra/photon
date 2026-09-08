<script setup lang="ts">
// Shared chrome for every `/infra/:host/*` route: AppShell, the back button that returns to the
// host list's own history entry, the sub-nav, and the per-host Related menu. Same reason
// ServiceScaffold exists — one copy of the header, not one per tab.
import { useRouter } from 'vue-router'
import { ArrowLeft } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import HostSubNav from '@/components/infra/HostSubNav.vue'
import { Button } from '@/components/ui/button'
import { backTo } from '@/lib/core/useBackTo'
import { api } from '@/lib/core/api'

const props = defineProps<{
  host: string
  /** Appended after `Infrastructure › {host}` — the tab's own name. */
  crumbSuffix?: string
}>()

const router = useRouter()
function back(): void {
  backTo(router, '/infra')
}
</script>

<template>
  <AppShell :mock="api.mock" :crumb="`Infrastructure › ${props.host}${crumbSuffix ? ' › ' + crumbSuffix : ''}`">
    <template #lead>
      <Button data-testid="back-to-hosts" size="icon" variant="ghost" class="size-7 shrink-0" aria-label="Back to hosts" @click="back">
        <ArrowLeft class="size-4" />
      </Button>
    </template>
    <template #toolbar>
      <div class="flex w-full items-center gap-3">
        <HostSubNav v-if="host" :host="host" class="shrink-0" />
        <div v-if="$slots.search" class="min-w-0 flex-1"><slot name="search" /></div>
      </div>
    </template>
    <template #actions>
      <div class="flex items-center gap-2">
        <slot name="actions" />
        <RelatedMenu :entity="{ kind: 'host', fields: { host } }" />
      </div>
    </template>
    <slot />
  </AppShell>
</template>
