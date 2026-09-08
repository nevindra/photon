<script setup lang="ts">
// One look for a role wherever it is shown read-only: brand tint for Admin (the role that can
// change the rules), a neutral chip for the two below it. Solid colour is not spent here — a
// role is a fact about a person, not an alarm.
import { Badge } from '@/components/ui/badge'
import { ROLE_META, type Role } from '@/lib/deploy/roles'

defineProps<{ role: Role }>()
</script>

<template>
  <Badge :variant="role === 'admin' ? 'brand' : 'outline'" :data-role="role">{{ ROLE_META[role].label }}</Badge>
</template>
