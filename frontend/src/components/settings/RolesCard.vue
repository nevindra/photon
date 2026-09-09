<script setup lang="ts">
// What each role may do, in the words an admin would use. Shown beside the table so the role
// column is never a word someone has to guess the weight of. Ranked top-down: each row may do
// everything the row above it may.
import { ShieldCheck } from 'lucide-vue-next'
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '@/components/ui/card'
import RoleBadge from './RoleBadge.vue'
import { ROLES, ROLE_META } from '@/lib/deploy/roles'

const ranked = [...ROLES].reverse()
</script>

<template>
  <Card data-testid="roles-card">
    <CardHeader class="pb-3">
      <div class="flex items-center gap-2">
        <span class="flex size-8 items-center justify-center rounded-lg bg-muted text-muted-foreground">
          <ShieldCheck class="size-4" />
        </span>
        <CardTitle>Roles</CardTitle>
      </div>
      <CardDescription>Each role includes everything below it. Only an admin hands them out.</CardDescription>
    </CardHeader>
    <CardContent class="pt-0">
      <ol class="flex flex-col divide-y divide-border">
        <li v-for="r in ranked" :key="r" class="flex items-start gap-3 py-2.5 first:pt-0 last:pb-0">
          <RoleBadge :role="r" class="mt-0.5 w-[4.5rem] justify-center" />
          <p class="text-xs leading-snug text-muted-foreground">{{ ROLE_META[r].summary }}</p>
        </li>
      </ol>
    </CardContent>
  </Card>
</template>
