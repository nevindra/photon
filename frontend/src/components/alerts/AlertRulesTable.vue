<script setup lang="ts">
// AlertsView's "rules" tab body. Self-fetches `useRules()`/`useChannels()` (like AlertStatBand, and
// unlike UptimeDashboard which owns `useMonitors()` at the view level) since AlertsView switches
// tabs without unmounting the stat band's own queries — each tab body is independently responsible
// for its data. Mirrors uptime/MonitorTable.vue's plain-Table-primitives shape (no TanStack Table:
// this list is small, unpaginated, and never virtualized).
import { computed } from 'vue'
import { Plus } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Table, TableHeader, TableBody, TableRow, TableHead } from '@/components/ui/table'
import { EmptyState } from '@/components/ui/empty-state'
import { Spinner } from '@/components/ui/spinner'
import { useRules, useChannels } from '@/lib/alertsQueries'
import AlertRuleRow from './AlertRuleRow.vue'
import type { AlertRule } from '@/lib/core/api'

defineEmits<{ 'open-create': []; edit: [rule: AlertRule] }>()

const rulesQuery = useRules()
const channelsQuery = useChannels()
const rules = computed(() => rulesQuery.data.value ?? [])
const channels = computed(() => channelsQuery.data.value ?? [])
const isLoading = computed(() => rulesQuery.isLoading.value)
const isError = computed(() => rulesQuery.isError.value)
</script>

<template>
  <div>
    <div class="mb-3 flex justify-end">
      <Button size="sm" data-testid="alert-new-rule" @click="$emit('open-create')">
        <Plus class="mr-1.5 size-3.5" />
        New alert
      </Button>
    </div>

    <p v-if="isLoading" class="text-sm text-muted-foreground"><Spinner size="sm">Loading…</Spinner></p>
    <p v-else-if="isError" class="text-sm text-destructive">Failed to load alert rules.</p>
    <EmptyState
      v-else-if="!rules.length"
      title="No alert rules yet"
      description="Create your first rule to get notified when a condition is met."
    />
    <div v-else class="overflow-x-auto rounded-lg border border-border bg-card shadow-1">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="text-left">Status</TableHead>
            <TableHead class="text-left">Alert</TableHead>
            <TableHead class="text-left">Signal</TableHead>
            <TableHead class="text-left">Condition</TableHead>
            <TableHead class="text-left">For</TableHead>
            <TableHead class="text-left">Channels</TableHead>
            <TableHead class="text-right"></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <AlertRuleRow
            v-for="rule in rules"
            :key="rule.id"
            :rule="rule"
            :channels="channels"
            @edit="$emit('edit', $event)"
          />
        </TableBody>
      </Table>
    </div>
  </div>
</template>
