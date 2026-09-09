<script setup lang="ts">
// `/infra/:host/containers` — the host's Docker inventory (read-only): containers grouped by
// compose project, images, volumes, networks, daemon events, and a per-container drawer with
// logs / stats / details. Own tab, own queries: the Overview never waits on an agent read.
import HostScaffold from '@/components/infra/HostScaffold.vue'
import HostContainersPanel from '@/components/docker/HostContainersPanel.vue'
import { useRouteParam } from '@/lib/services/useServiceParam'

const host = useRouteParam('host')
</script>

<template>
  <HostScaffold :host="host" crumb-suffix="Containers">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="host-containers-view">
      <HostContainersPanel :host="host" />
    </main>
  </HostScaffold>
</template>
