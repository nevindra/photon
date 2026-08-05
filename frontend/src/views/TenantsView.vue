<script setup lang="ts">
// `/tenants` (Manage group): the federation tenant registry, promoted out of the /data tabs into
// its own page. The registry table + manage dialog live in DataTenants (kept under components/data/
// to avoid churning its tests); this view is just the AppShell wrapper.
import AppShell from '@/components/common/AppShell.vue'
import DataTenants from '@/components/data/DataTenants.vue'
import { api } from '@/lib/core/api'
</script>

<template>
  <AppShell :mock="api.mock" crumb="Tenants">
    <section class="flex min-h-0 flex-1 flex-col overflow-y-auto px-5 pt-5 pb-6">
      <DataTenants />
    </section>
  </AppShell>
</template>
