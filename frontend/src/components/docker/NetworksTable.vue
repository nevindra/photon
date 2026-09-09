<script setup lang="ts">
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { Badge } from '@/components/ui/badge'
import { EmptyState } from '@/components/ui/empty-state'
import type { NetworkInfo } from '@/lib/core/api'

defineProps<{ networks: NetworkInfo[] }>()
</script>

<template>
  <section class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="networks-table">
    <Table container-class="overflow-x-auto" class="text-sm">
      <TableHeader>
        <TableRow class="text-[10px] font-medium uppercase tracking-wider text-muted-foreground hover:bg-transparent">
          <TableHead>Network</TableHead>
          <TableHead class="w-24">Driver</TableHead>
          <TableHead class="w-24">Scope</TableHead>
          <TableHead class="w-40">Subnet</TableHead>
          <TableHead class="w-28 text-right">Containers</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="n in networks" :key="n.id" data-testid="network-row" class="border-border/60">
          <TableCell class="py-2">
            <span class="font-mono text-xs text-foreground">{{ n.name }}</span>
            <Badge v-if="n.internal" variant="outline" class="ml-1.5">internal</Badge>
          </TableCell>
          <TableCell class="py-2 text-xs text-muted-foreground">{{ n.driver }}</TableCell>
          <TableCell class="py-2 text-xs text-muted-foreground">{{ n.scope }}</TableCell>
          <TableCell class="py-2 font-mono text-xs text-muted-foreground">{{ n.subnet ?? '—' }}</TableCell>
          <TableCell class="py-2 text-right tabular-nums" :class="n.containers ? 'text-foreground' : 'text-muted-foreground'">{{ n.containers }}</TableCell>
        </TableRow>
      </TableBody>
    </Table>
    <EmptyState v-if="!networks.length" title="No networks on this host" class="h-auto py-10" />
  </section>
</template>
