<script setup lang="ts">
// Initials in a circle. The signed-in user gets the brand tint so "which one is me" is answered
// at a glance without reading; everyone else is neutral — no per-user colours, the palette has
// one accent and it is not a name tag.
import { cn } from '@/lib/core/utils'
import { initials } from '@/lib/core/users'

defineProps<{ username: string; me?: boolean; size?: 'sm' | 'md' }>()
</script>

<template>
  <span
    :class="
      cn(
        'inline-flex shrink-0 select-none items-center justify-center rounded-full font-semibold tracking-wide',
        size === 'md' ? 'size-10 text-[13px]' : 'size-8 text-[11px]',
        me ? 'bg-brand text-brand-foreground' : 'bg-muted text-foreground ring-1 ring-inset ring-border',
      )
    "
    aria-hidden="true"
  >
    {{ initials(username) }}
  </span>
</template>
