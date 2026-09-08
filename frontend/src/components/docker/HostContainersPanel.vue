<script setup lang="ts">
// The Containers tab body. Owns the inventory query, the view switch (containers / images /
// volumes / networks — `?view=`), the text filter (`?q=`) and the selected container (`?c=`), and
// hands each view its rows. An offline host shows its LAST inventory under a plain banner rather
// than an empty page: what the agent saw before it went quiet is still the best answer to "what
// runs there", as long as it is labelled as such.
import { computed, ref, watch } from 'vue'
import { useNow } from '@vueuse/core'
import { RouterLink } from 'vue-router'
import { Search, WifiOff, Hourglass } from 'lucide-vue-next'
import { Input } from '@/components/ui/input'
import { Segmented, SegmentedItem } from '@/components/ui/segmented'
import { Spinner } from '@/components/ui/spinner'
import { EmptyState } from '@/components/ui/empty-state'
import { Card } from '@/components/ui/card'
import ContainerKpis from './ContainerKpis.vue'
import ContainerGroup from './ContainerGroup.vue'
import ImagesTable from './ImagesTable.vue'
import VolumesTable from './VolumesTable.vue'
import NetworksTable from './NetworksTable.vue'
import DockerEventsFeed from './DockerEventsFeed.vue'
import ContainerDrawer from './ContainerDrawer.vue'
import ContainerActionMenu from './ContainerActionMenu.vue'
import ActionConfirmDialog from './ActionConfirmDialog.vue'
import PruneDialog from './PruneDialog.vue'
import ImageActionDialog from './ImageActionDialog.vue'
import VolumeRemoveDialog from './VolumeRemoveDialog.vue'
import HostActionsStrip from './HostActionsStrip.vue'
import DeployUnavailable from '@/components/deploy/DeployUnavailable.vue'
import { useHostInventory, useDockerEvents, useAgent, useHostActions, useCreateAction } from '@/lib/deploy/deployQueries'
import { useUrlParam } from '@/lib/core/useUrlState'
import { groupByProject, matchesFilter } from '@/lib/deploy/containers'
import { ACTION_META } from '@/lib/deploy/actions'
import { ageMs } from '@/lib/deploy/audit'
import { can } from '@/lib/core/auth'
import { permissionHint } from '@/lib/deploy/roles'
import type { ActionArgs, ActionKind, Container, ImageInfo, VolumeInfo } from '@/lib/core/api'
import { isDeployUnavailable } from '@/lib/deploy/availability'

const props = defineProps<{ host: string }>()

const now = useNow({ interval: 1000 })
const q = useHostInventory(() => props.host)
const inv = computed(() => q.data.value ?? null)
const loading = computed(() => q.isLoading.value)
const eventsQ = useDockerEvents(() => props.host)
const events = computed(() => eventsQ.data.value?.events ?? [])

// --- Lifecycle actions -----------------------------------------------------------------------
// Allowed when the ROLE may (operator+) AND the agent admits to `lifecycle`. Either "no" is a
// sentence on the disabled control, never a missing button. Offline is not a "no": the action
// queues and the confirm dialog says so.
const agentQ = useAgent(() => props.host)
const agent = computed(() => agentQ.data.value ?? null)
const blocked = computed<string | null>(() => {
  if (!can('lifecycle')) return permissionHint('lifecycle')
  if (agent.value && !agent.value.capabilities.includes('lifecycle')) return 'The agent on this host was installed without --docker-lifecycle'
  return null
})
// Exec is the highest-risk feature: admin role AND the agent's own `exec` capability.
const execBlocked = computed<string | null>(() => {
  if (!can('exec')) return permissionHint('exec')
  if (agent.value && !agent.value.capabilities.includes('exec')) return 'The agent on this host was installed without --docker-exec'
  if (agent.value && agent.value.status !== 'online') return 'The host is offline — a shell needs a live agent'
  return null
})
const actionsQ = useHostActions(() => props.host)
const actions = computed(() => actionsQ.data.value?.actions ?? [])
const busy = computed<Record<string, ActionKind>>(() => {
  const m: Record<string, ActionKind> = {}
  for (const a of actions.value) if ((a.status === 'queued' || a.status === 'running') && a.container_id) m[a.container_id] = a.kind
  return m
})
const create = useCreateAction(() => props.host)
const confirmOpen = ref(false)
const confirmKind = ref<ActionKind | null>(null)
const confirmTarget = ref<Container | null>(null)
const pruneOpen = ref(false)
const imageOpen = ref(false)
const imageKind = ref<'image_pull' | 'image_remove' | null>(null)
const imageTarget = ref<ImageInfo | null>(null)
function actImage(img: ImageInfo, kind: 'image_pull' | 'image_remove'): void {
  if (blocked.value) return
  imageTarget.value = img
  imageKind.value = kind
  imageOpen.value = true
}
async function onImageConfirm(): Promise<void> {
  if (!imageTarget.value || !imageKind.value) return
  const res = await create.mutateAsync({ kind: imageKind.value, image: imageTarget.value.repo_tags[0] ?? imageTarget.value.id })
  if (res.ok) imageOpen.value = false
}
const volumeOpen = ref(false)
const volumeTarget = ref<VolumeInfo | null>(null)
function removeVolume(v: VolumeInfo): void {
  if (blocked.value) return
  volumeTarget.value = v
  volumeOpen.value = true
}
async function onVolumeConfirm(): Promise<void> {
  if (!volumeTarget.value) return
  const res = await create.mutateAsync({ kind: 'volume_remove', volume: volumeTarget.value.name })
  if (res.ok) volumeOpen.value = false
}

function act(c: Container, kind: ActionKind): void {
  if (blocked.value) return
  confirmTarget.value = c
  confirmKind.value = kind
  // A start needs no ceremony; everything else states what it does and asks once.
  if (ACTION_META[kind].risk === 'none') void run(c, kind, {})
  else confirmOpen.value = true
}
async function run(c: Container | null, kind: ActionKind, args: ActionArgs): Promise<void> {
  const res = await create.mutateAsync({ kind, container_id: c?.id, args })
  if (res.ok) {
    confirmOpen.value = false
    pruneOpen.value = false
  }
}
function onConfirm(args: ActionArgs): void {
  if (confirmTarget.value && confirmKind.value) void run(confirmTarget.value, confirmKind.value, args)
}
function onPrune(args: ActionArgs): void {
  void run(null, 'prune', args)
}

type View = 'containers' | 'images' | 'volumes' | 'networks'
const VIEWS: View[] = ['containers', 'images', 'volumes', 'networks']
const view = ref<string | null>('containers')
const filter = ref<string | null>(null)
const selectedName = ref<string | null>(null)
useUrlParam('view', view, { valid: (v) => (VIEWS as string[]).includes(v), fallback: 'containers' })
useUrlParam('q', filter)
useUrlParam('c', selectedName)

const filtered = computed(() => (inv.value?.containers ?? []).filter((c) => matchesFilter(c, filter.value ?? '')))
const groups = computed(() => groupByProject(filtered.value))
const counts = computed(() => ({
  containers: inv.value?.containers.length ?? 0,
  images: inv.value?.images.length ?? 0,
  volumes: inv.value?.volumes.length ?? 0,
  networks: inv.value?.networks.length ?? 0,
}))

const selected = computed<Container | null>(() => (inv.value?.containers ?? []).find((c) => c.name === selectedName.value || c.id === selectedName.value) ?? null)
const drawerOpen = ref(false)
watch(selected, (c) => { if (c) drawerOpen.value = true }, { immediate: true })
function open(c: Container): void {
  selectedName.value = c.name
  drawerOpen.value = true
}
function onDrawer(o: boolean): void {
  drawerOpen.value = o
  if (!o) selectedName.value = null
}
function openByName(name: string): void {
  const c = inv.value?.containers.find((x) => x.name === name)
  if (c) open(c)
}
</script>

<template>
  <DeployUnavailable v-if="isDeployUnavailable(q.error.value)" what="Container management" />
  <div v-else-if="loading && !inv" class="flex flex-1 items-center justify-center py-16"><Spinner size="lg">Reading the inventory…</Spinner></div>

  <Card v-else-if="!inv" class="py-4" data-testid="containers-not-managed">
    <EmptyState title="No agent manages this host" description="Container inventory comes from a photon-agent enrolled with the Read capability. Enroll it on the Agent tab.">
      <RouterLink :to="`/infra/${encodeURIComponent(host)}/agent`" class="mt-3 text-sm text-brand hover:underline">Open the Agent tab →</RouterLink>
    </EmptyState>
  </Card>

  <div v-else class="flex flex-col gap-4" data-testid="containers-panel">
    <!-- Offline is stated, not shouted: a warning tone, because an unanswered host is the normal
         case on-prem and nothing here has failed. -->
    <div v-if="inv.stale" class="flex flex-wrap items-center gap-2 rounded-xl border px-4 py-2.5 text-sm" :class="inv.agent_status === 'offline' ? 'border-sev-warn/30 bg-sev-warn-soft' : 'border-border bg-muted'" data-testid="inventory-stale">
      <component :is="inv.agent_status === 'offline' ? WifiOff : Hourglass" class="size-4 shrink-0" :class="inv.agent_status === 'offline' ? 'text-sev-warn' : 'text-muted-foreground'" />
      <span class="font-medium" :class="inv.agent_status === 'offline' ? 'text-sev-warn' : 'text-foreground'">
        {{ inv.agent_status === 'offline' ? `The agent is offline — this is what it saw ${ageMs(inv.fetched_ms)}.` : 'Waiting for the agent’s first sync.' }}
      </span>
      <span class="text-xs text-muted-foreground">Nothing below is live.</span>
      <RouterLink :to="`/infra/${encodeURIComponent(host)}/agent`" class="ml-auto text-xs text-brand hover:underline">Agent tab →</RouterLink>
    </div>

    <ContainerKpis :inventory="inv" :blocked="blocked" @prune="pruneOpen = true" />

    <!-- The inventory card: the view switch, filter and "as of" live in ITS header, on the thing
         they control, and the compose-project groups stack inside it. -->
    <section class="overflow-hidden rounded-xl border border-border bg-card shadow-1" data-testid="inventory-card">
      <header class="flex flex-wrap items-center gap-2 px-3 py-2">
        <Segmented :model-value="view ?? 'containers'" data-testid="inventory-view" @update:model-value="view = String($event)">
          <SegmentedItem v-for="v in VIEWS" :key="v" :value="v" class="capitalize">{{ v }} <span class="ml-1 tabular-nums text-muted-foreground">{{ counts[v] }}</span></SegmentedItem>
        </Segmented>
        <div v-if="view === 'containers'" class="relative ml-auto w-72 max-w-full">
          <Search class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input :model-value="filter ?? ''" placeholder="Filter by name, image, project, label…" class="h-8 pl-8 text-xs" data-testid="container-filter" @update:model-value="filter = String($event) || null" />
        </div>
        <span class="text-xs text-muted-foreground" :class="view !== 'containers' && 'ml-auto'">as of {{ ageMs(inv.fetched_ms, now.getTime()) }}</span>
      </header>

      <template v-if="view === 'containers'">
        <ContainerGroup v-for="g in groups" :key="g.project ?? '__standalone'" :group="g" :host="host" embedded :selected-id="selected?.id" :blocked="blocked" :busy="busy" :stale="inv.stale" :as-of="ageMs(inv.fetched_ms)" @open="open" @act="act" />
        <EmptyState v-if="!groups.length" :title="filter ? 'No container matches the filter' : 'No containers on this host'" :description="filter ? 'Try a shorter name, an image, or a compose project.' : 'The daemon reports nothing running or stopped.'" class="border-t border-border py-8" />
      </template>
      <div v-else class="border-t border-border [&>section]:rounded-none [&>section]:border-0 [&>section]:shadow-none">
        <ImagesTable v-if="view === 'images'" :images="inv.images" :blocked="blocked" @act="actImage" />
        <VolumesTable v-else-if="view === 'volumes'" :volumes="inv.volumes" :blocked="blocked" @remove="removeVolume" />
        <NetworksTable v-else :networks="inv.networks" />
      </div>
    </section>

    <!-- What Photon did beside what the daemon saw; the feed takes the whole row when there
         is nothing Photon did lately. -->
    <div v-if="view === 'containers'" class="grid items-start gap-4" :class="actions.length ? 'lg:grid-cols-[2fr_3fr]' : ''">
      <HostActionsStrip :actions="actions" :host="host" />
      <DockerEventsFeed :host="host" :events="events" @open="openByName" />
    </div>

    <ContainerDrawer :open="drawerOpen && !!selected" :host="host" :container="selected" :exec-blocked="execBlocked" @update:open="onDrawer">
      <template #actions>
        <ContainerActionMenu v-if="selected" :container="selected" :blocked="blocked" :busy="busy[selected.id] ?? null" size="default" @act="act(selected!, $event)" />
        <span v-if="blocked" class="text-xs text-muted-foreground">{{ blocked }}</span>
      </template>
    </ContainerDrawer>
    <ActionConfirmDialog :open="confirmOpen" :kind="confirmKind" :container="confirmTarget" :host-status="inv.agent_status" :pending="create.isPending.value" @update:open="confirmOpen = $event" @confirm="onConfirm" />
    <ImageActionDialog :open="imageOpen" :kind="imageKind" :image="imageTarget" :host-status="inv.agent_status" :pending="create.isPending.value" @update:open="imageOpen = $event" @confirm="onImageConfirm" />
    <VolumeRemoveDialog :open="volumeOpen" :volume="volumeTarget" :host-status="inv.agent_status" :pending="create.isPending.value" @update:open="volumeOpen = $event" @confirm="onVolumeConfirm" />
    <PruneDialog :open="pruneOpen" :host="host" :host-status="inv.agent_status" :pending="create.isPending.value" @update:open="pruneOpen = $event" @confirm="onPrune" />
  </div>
</template>
