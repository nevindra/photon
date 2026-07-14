<script setup>
import MonitorRow from './MonitorRow.vue'
import { Table, TableHeader, TableBody, TableRow, TableHead } from '@/components/ui/table'
defineProps({ monitors: { type: Array, default: () => [] } })
defineEmits(['select'])
</script>

<template>
  <div class="overflow-x-auto rounded-lg border border-border bg-card shadow-1">
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead class="text-left">Monitor</TableHead>
          <TableHead class="text-left">Status</TableHead>
          <TableHead class="text-right">Uptime 24h</TableHead>
          <TableHead class="text-right">Latency</TableHead>
          <TableHead class="text-left">Type</TableHead>
          <TableHead class="text-left">Heartbeats</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <MonitorRow v-for="m in monitors" :key="m.id" :monitor="m" @select="$emit('select', $event)" />
      </TableBody>
    </Table>
  </div>
</template>
