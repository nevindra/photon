<script setup lang="ts">
// Shared chrome for every `/services/:service/*` route: the AppShell, the back button that returns
// to the list's own history entry, the sub-nav, and the per-service actions.
//
// It exists so a new sub-route is one view file, not one view file plus forty lines of copied
// header. Three copies of this chrome is how the back button, the Related menu and the Apdex
// control end up subtly different on the tab you visit least.
import ServiceSubNav from './ServiceSubNav.vue'
import ApdexThresholdControl from './ApdexThresholdControl.vue'
import AppShell from '@/components/common/AppShell.vue'
import RelatedMenu from '@/components/common/RelatedMenu.vue'
import { Button } from '@/components/ui/button'
import { ArrowLeft } from 'lucide-vue-next'
import { useRouter } from 'vue-router'
import { backTo } from '@/lib/core/useBackTo'
import { api } from '@/lib/core/api'

const props = defineProps<{
  service: string
  /** Appended after `Backend › {service}` — the sub-route's own name. */
  crumbSuffix?: string
}>()

const router = useRouter()

// Same round-trip preservation as the trace detail's back button (`lib/core/useBackTo.ts`):
// return to the LIST's own history entry so its sort/search state survives a drill-in.
function back(): void {
  backTo(router, '/services')
}
</script>

<template>
  <AppShell
    :mock="api.mock"
    :crumb="`Backend › ${service}${crumbSuffix ? ' › ' + crumbSuffix : ''}`"
  >
    <template #lead>
      <Button
        data-testid="back-to-services"
        size="icon"
        variant="ghost"
        class="size-7 shrink-0"
        aria-label="Back to services"
        @click="back"
      >
        <ArrowLeft class="size-4" />
      </Button>
    </template>

    <!-- The sub-nav and an optional per-tab search share the ContextBar's ONE fixed-height
         search region, laid out side by side (nav shrink-to-content, search filling the rest).
         Stacking them would overflow the bar's fixed height — same call RumErrorsView made. -->
    <template #toolbar>
      <div class="flex w-full items-center gap-3">
        <ServiceSubNav v-if="service" :service="service" class="shrink-0" />
        <div v-if="$slots.search" class="min-w-0 flex-1">
          <slot name="search" />
        </div>
      </div>
    </template>

    <template #actions>
      <div v-if="service" class="flex items-center gap-2">
        <slot name="actions" />
        <RelatedMenu :entity="{ kind: 'service', fields: { service } }" />
        <ApdexThresholdControl :service="service" />
      </div>
    </template>

    <slot />
  </AppShell>
</template>
