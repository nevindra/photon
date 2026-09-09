<script setup lang="ts">
// `/infra/:host/agent` — the host's managed-agent tab. The scaffold owns the chrome; the panel
// owns the queries, so the Overview tab never waits on an agent read.
import HostScaffold from '@/components/infra/HostScaffold.vue'
import HostAgentPanel from '@/components/infra/HostAgentPanel.vue'
import { useRouteParam } from '@/lib/services/useServiceParam'

const host = useRouteParam('host')
</script>

<template>
  <HostScaffold :host="host" crumb-suffix="Agent">
    <main class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" data-testid="host-agent-view">
      <HostAgentPanel :host="host" />
    </main>
  </HostScaffold>
</template>
