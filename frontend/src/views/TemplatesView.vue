<script setup lang="ts">
// `/deploy/templates` — the catalog: what we ship, parameterized and versioned. A grid of cards
// (the thing an operator scans), filtered by search, category and source. Deploy starts from
// the card; Publish and Sources are admin doors in the header.
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { Plus, Search, Globe } from 'lucide-vue-next'
import AppShell from '@/components/common/AppShell.vue'
import TemplateCard from '@/components/deploy/TemplateCard.vue'
import DeployTemplateDialog from '@/components/deploy/DeployTemplateDialog.vue'
import RegistriesDialog from '@/components/deploy/RegistriesDialog.vue'
import PublishVersionDialog from '@/components/deploy/PublishVersionDialog.vue'
import GatedButton from '@/components/deploy/GatedButton.vue'
import { Input } from '@/components/ui/input'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { Card } from '@/components/ui/card'
import { api, type Template } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import { useUrlParam } from '@/lib/core/useUrlState'
import { useTemplates } from '@/lib/deploy/deployQueries'
import { cn } from '@/lib/core/utils'

const router = useRouter()
const q = useTemplates()
const all = computed(() => q.data.value?.templates ?? [])
const loading = computed(() => q.isLoading.value)
const unavailable = computed(() => isDeployUnavailable(q.error.value))

const text = ref<string | null>(null)
const category = ref<string | null>(null)
const source = ref<string | null>(null)
useUrlParam('q', text)
useUrlParam('cat', category)
useUrlParam('source', source, { valid: (v) => v === 'local' || v === 'remote' })

const categories = computed(() => {
  const counts = new Map<string, number>()
  for (const t of all.value) for (const c of t.categories) counts.set(c, (counts.get(c) ?? 0) + 1)
  return Array.from(counts.entries()).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
})
const rows = computed(() => {
  const needle = (text.value ?? '').trim().toLowerCase()
  return all.value.filter((t) => {
    if (category.value && !t.categories.includes(category.value)) return false
    if (source.value === 'local' && t.source !== 'local') return false
    if (source.value === 'remote' && t.source === 'local') return false
    if (needle && ![t.name, t.description, t.id, ...t.categories].join('\n').toLowerCase().includes(needle)) return false
    return true
  })
})

const deployOpen = ref(false)
const deployTemplate = ref<Template | null>(null)
const registriesOpen = ref(false)
const publishOpen = ref(false)
function deploy(t: Template): void {
  deployTemplate.value = t
  deployOpen.value = true
}
function open(t: Template): void {
  router.push(`/deploy/templates/${encodeURIComponent(t.id)}`)
}
</script>

<template>
  <AppShell :mock="api.mock" crumb="Deploy › Templates" :time-picker="false">
    <template #actions>
      <GatedButton perm="templates" size="sm" variant="outline" data-testid="open-registries" @click="registriesOpen = true"><Globe class="size-4" /> Sources</GatedButton>
      <GatedButton perm="templates" size="sm" data-testid="new-template" @click="publishOpen = true"><Plus class="size-4" /> New template</GatedButton>
    </template>
    <section class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 pt-5 pb-6" data-testid="templates-view">
      <header>
        <h1 class="text-xl font-semibold text-foreground">Templates</h1>
        <p class="text-sm text-muted-foreground">The catalog of what we ship: versioned, parameterized recipes. Deploy one to a host; every deploy is observable from its first second.</p>
      </header>

      <div class="flex flex-wrap items-center gap-2" data-testid="template-filters">
        <div class="relative min-w-[14rem] flex-1">
          <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input :model-value="text ?? ''" placeholder="Search templates…" class="h-8 pl-8 text-xs" data-testid="template-search" @update:model-value="text = String($event) || null" />
        </div>
        <Segmented :model-value="source ?? ''" class="h-8" data-testid="template-source" @update:model-value="source = $event ? String($event) : null">
          <SegmentedItem value="" class="px-2.5 text-xs">All</SegmentedItem>
          <SegmentedItem value="local" class="px-2.5 text-xs">Ours</SegmentedItem>
          <SegmentedItem value="remote" class="px-2.5 text-xs">Imported</SegmentedItem>
        </Segmented>
      </div>
      <div v-if="categories.length" class="flex flex-wrap gap-1.5" data-testid="template-categories">
        <button v-for="[c, n] in categories" :key="c" type="button" :data-category="c" :class="cn('rounded-full border px-2.5 py-0.5 text-xs transition-colors', category === c ? 'border-brand/35 bg-brand-soft text-brand' : 'border-border bg-card text-muted-foreground hover:text-foreground')" @click="category = category === c ? null : c">
          {{ c }} <span class="tabular-nums opacity-70">{{ n }}</span>
        </button>
      </div>

      <DeployUnavailable v-if="unavailable" what="The template catalog" />
      <div v-else-if="loading && !all.length" class="flex flex-1 items-center justify-center py-16"><Spinner size="lg">Loading templates…</Spinner></div>
      <div v-else-if="rows.length" class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3" data-testid="template-grid">
        <TemplateCard v-for="t in rows" :key="t.id" :template="t" @open="open" @deploy="deploy" />
      </div>
      <Card v-else class="py-4"><EmptyState :title="all.length ? 'No template matches' : 'No templates yet'" :description="all.length ? 'Try another search or category.' : 'Publish one, or import a registry under Sources.'" class="py-8" /></Card>

      <DeployTemplateDialog :open="deployOpen" :template-id="deployTemplate?.id ?? null" @update:open="deployOpen = $event" />
      <RegistriesDialog :open="registriesOpen" @update:open="registriesOpen = $event" />
      <PublishVersionDialog :open="publishOpen" @update:open="publishOpen = $event" />
    </section>
  </AppShell>
</template>
