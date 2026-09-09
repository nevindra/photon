<script setup lang="ts">
// A Button that knows which role it needs. When the signed-in user lacks it, the button stays
// visible but disabled and explains itself on hover ("Requires the Operator role") — a control
// that vanishes reads as a missing feature, one that refuses reads as a policy. Everything else
// (variant, size, class, click) passes straight through to `ui/button`.
import { computed, useAttrs } from 'vue'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip'
import { can } from '@/lib/core/auth'
import { permissionHint, type Permission } from '@/lib/deploy/roles'

defineOptions({ inheritAttrs: false })

const props = defineProps<{
  perm: Permission
  disabled?: boolean
  /** Why it is disabled for a reason other than the role ("installed without --docker-apply"). */
  reason?: string | null
}>()

const attrs = useAttrs()
const allowed = computed(() => can(props.perm))
const hint = computed(() => permissionHint(props.perm))
</script>

<template>
  <Tooltip v-if="!allowed">
    <TooltipTrigger as-child>
      <!-- A disabled button swallows pointer events, so the tooltip anchors on a wrapper. -->
      <span class="inline-flex" tabindex="0" :data-perm="perm" data-gated="true">
        <Button v-bind="attrs" disabled aria-disabled="true"><slot /></Button>
      </span>
    </TooltipTrigger>
    <TooltipContent>{{ hint }}</TooltipContent>
  </Tooltip>
  <Tooltip v-else-if="disabled && reason">
    <TooltipTrigger as-child>
      <span class="inline-flex" tabindex="0" :data-perm="perm" data-gated="reason">
        <Button v-bind="attrs" disabled aria-disabled="true"><slot /></Button>
      </span>
    </TooltipTrigger>
    <TooltipContent>{{ reason }}</TooltipContent>
  </Tooltip>
  <Button v-else v-bind="attrs" :disabled="disabled" :data-perm="perm"><slot /></Button>
</template>
