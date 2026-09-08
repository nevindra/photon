// TanStack Query wrappers over the Docker fleet-management api methods (agents, enrollment,
// bootstrap jobs, audit). Same contract as `tenantsQueries.ts`: mutations never reject — the api
// methods return `{ ok, error }` — so `onSuccess` is the one place that invalidates and toasts.
//
// Agents poll every 15s (a fleet board is a live surface); a bootstrap job polls every second
// while it runs and stops the moment it settles, so a finished stepper costs nothing.
import { computed, ref, toValue, watch, type MaybeRefOrGetter } from 'vue'
import { useQuery, useMutation, useQueryClient, keepPreviousData } from '@tanstack/vue-query'
import {
  api,
  type AgentPatch,
  type AuditFilter,
  type AuditResult,
  type BootstrapJob,
  type ActionInput,
  type ActionResult,
  type ActionsResult,
  type BootstrapStartResult,
  type ContainerLogLine,
  type ContainerLogsResult,
  type ContainerStats,
  type DeployOptions,
  type DiscoveredProjectsResult,
  type DockerEventsResult,
  type DryRunResult,
  type ImportResult,
  type RegistriesResult,
  type RegistryCredentialInput,
  type RegistryCredentialResult,
  type RegistryCredentialsResult,
  type RegistryInput,
  type Rollout,
  type RolloutActionKind,
  type RolloutInput,
  type RolloutResult,
  type RolloutsResult,
  type RegistryResult,
  type Template,
  type TemplateDeployInput,
  type TemplateVersion,
  type TemplateVersionInput,
  type TemplateVersionResult,
  type TemplatesResult,
  type EnrollInput,
  type HostInventory,
  type EnrollResult,
  type ManagedHost,
  type ManagedHostsResult,
  type MutationResult,
  type PrunePreview,
  type SshBootstrapInput,
  type Stack,
  type StackActionArgs,
  type StackActionKind,
  type StackFile,
  type StackInput,
  type StackResult,
  type StacksResult,
  type StackValidation,
} from '@/lib/core/api'
import { toast } from '@/components/ui/toast'
import { ACTION_META } from '@/lib/deploy/actions'
import { isDeployUnavailable } from '@/lib/deploy/availability'
import { tenant as activeTenant } from '@/lib/core/context'

// A subsystem that is not enabled is not a transient failure: never retry it.
const deployRetry = (failures: number, err: unknown): boolean => !isDeployUnavailable(err) && failures < 2


export const agentsKey = (): string[] => ['agents']
export const agentKey = (host: string): string[] => ['agents', 'host', host]
// NOT under `agentsKey()`: invalidating the agents list must never re-trigger a job poll.
export const bootstrapKey = (id: string): string[] => ['bootstrap-job', id]
export const auditKey = (): string[] => ['audit']

export function useAgents() {
  return useQuery({
    retry: deployRetry,
    queryKey: agentsKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<ManagedHostsResult> => api.agents({ signal }),
    refetchInterval: 15_000,
  })
}

export function useAgent(host: MaybeRefOrGetter<string>) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => agentKey(toValue(host))),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<ManagedHost | null> => api.agent(toValue(host), { signal }),
    enabled: computed(() => !!toValue(host)),
    refetchInterval: 15_000,
    placeholderData: keepPreviousData,
  })
}

function invalidateAgents(qc: ReturnType<typeof useQueryClient>): void {
  qc.invalidateQueries({ queryKey: agentsKey() })
  qc.invalidateQueries({ queryKey: auditKey() })
}

export function useEnrollAgent() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: EnrollInput): Promise<EnrollResult> => api.enrollAgent(input),
    onSuccess: (res: EnrollResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't enroll host", description: res.error ?? 'Please try again.' })
        return
      }
      invalidateAgents(qc)
      toast({ variant: 'success', title: `${res.host?.host ?? 'Host'} enrolled`, description: 'Install the agent with the snippet to finish.' })
    },
  })
}

export function useUpdateAgent() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ host, patch }: { host: string; patch: AgentPatch }): Promise<MutationResult & { host?: ManagedHost }> =>
      api.updateAgent(host, patch),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't update agent", description: res.error ?? 'Please try again.' })
        return
      }
      invalidateAgents(qc)
      toast({ variant: 'success', title: 'Agent updated' })
    },
  })
}

export function useRotateAgentToken() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (host: string): Promise<MutationResult & { token?: string }> => api.rotateAgentToken(host),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't rotate token", description: res.error ?? 'Please try again.' })
        return
      }
      invalidateAgents(qc)
      toast({ variant: 'success', title: 'Token rotated', description: 'The old token stops working on the next sync.' })
    },
  })
}

export function useDeleteAgent() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (host: string): Promise<MutationResult> => api.deleteAgent(host),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't remove agent", description: res.error ?? 'Please try again.' })
        return
      }
      invalidateAgents(qc)
      toast({ variant: 'success', title: 'Agent removed', description: 'Containers on the host keep running.' })
    },
  })
}

export function useRequestAgentUpgrade() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (host: string): Promise<MutationResult> => api.requestAgentUpgrade(host),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't schedule upgrade", description: res.error ?? 'Please try again.' })
        return
      }
      invalidateAgents(qc)
      toast({ variant: 'success', title: 'Upgrade scheduled', description: 'The agent fetches it on its next sync and restarts itself.' })
    },
  })
}

export function useStartBootstrap() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: SshBootstrapInput): Promise<BootstrapStartResult> => api.startBootstrap(input),
    onSuccess: (res: BootstrapStartResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't start the install", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: auditKey() })
    },
  })
}

// Polls once a second while the job runs; a settled job (done/failed) is fetched once and left.
// When it settles, the agents list and audit log are invalidated ONCE from a watcher — not from
// inside `refetchInterval`, where an invalidation that touched this query's own key would loop.
export function useBootstrapJob(id: MaybeRefOrGetter<string | null>) {
  const qc = useQueryClient()
  const q = useQuery({
    retry: deployRetry,
    queryKey: computed(() => bootstrapKey(toValue(id) ?? '')),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<BootstrapJob | null> => api.bootstrapJob(toValue(id)!, { signal }),
    enabled: computed(() => !!toValue(id)),
    refetchInterval: (query) => {
      const job = query.state.data as BootstrapJob | null | undefined
      return job && job.status !== 'running' ? false : 1_000
    },
  })
  watch(
    () => q.data.value?.status,
    (status, prev) => {
      if (status && status !== 'running' && prev === 'running') {
        qc.invalidateQueries({ queryKey: agentsKey() })
        qc.invalidateQueries({ queryKey: auditKey() })
      }
    },
  )
  return q
}

export function useAudit(filter: MaybeRefOrGetter<AuditFilter>) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => [...auditKey(), JSON.stringify(toValue(filter))]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<AuditResult> => api.audit(toValue(filter), { signal }),
    placeholderData: keepPreviousData,
    refetchInterval: 30_000,
  })
}

// --- Read-only Docker inventory ---------------------------------------------------------------

export const inventoryKey = (host: string): string[] => ['docker', 'inventory', host]

// The whole inventory in one read, polled every 15s like the other live host surfaces. Kept on
// screen across a refetch (`keepPreviousData`) so the table never flashes empty.
export function useHostInventory(host: MaybeRefOrGetter<string>) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => inventoryKey(toValue(host))),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<HostInventory | null> => api.hostInventory(toValue(host), { signal }),
    enabled: computed(() => !!toValue(host)),
    refetchInterval: 15_000,
    placeholderData: keepPreviousData,
  })
}

// Container logs: the tail, then — while `follow` is on — a 1s poll for "everything since the last
// line". No second port, no tunnel: the server drops the agent's poll hint to 1s while a log view
// is open, and the UI polls at the same cadence. Lines accumulate in the composable (capped) so
// the caller renders one growing list.
export function useContainerLogs(
  host: MaybeRefOrGetter<string>,
  id: MaybeRefOrGetter<string | null>,
  opts: { tail?: number; follow: MaybeRefOrGetter<boolean>; cap?: number } = { follow: false },
) {
  const lines = ref<ContainerLogLine[]>([])
  const truncated = ref(false)
  const cap = opts.cap ?? 2_000
  const tailQ = useQuery({
    retry: deployRetry,
    queryKey: computed(() => ['docker', 'logs', toValue(host), toValue(id) ?? '', 'tail']),
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<ContainerLogsResult> => {
      const res = await api.containerLogs(toValue(host), toValue(id)!, { tail: opts.tail ?? 200 }, { signal })
      lines.value = res.lines
      truncated.value = res.truncated
      return res
    },
    enabled: computed(() => !!toValue(host) && !!toValue(id)),
    gcTime: 0,
  })
  const lastTs = computed(() => (lines.value.length ? lines.value[lines.value.length - 1].ts_ms : null))
  const followQ = useQuery({
    retry: deployRetry,
    queryKey: computed(() => ['docker', 'logs', toValue(host), toValue(id) ?? '', 'follow']),
    queryFn: async ({ signal }: { signal: AbortSignal }): Promise<number> => {
      const since = lastTs.value
      if (since == null) return 0
      const res = await api.containerLogs(toValue(host), toValue(id)!, { since_ms: since }, { signal })
      if (res.lines.length) lines.value = [...lines.value, ...res.lines].slice(-cap)
      return res.lines.length
    },
    enabled: computed(() => !!toValue(host) && !!toValue(id) && !!toValue(opts.follow) && lastTs.value != null),
    refetchInterval: 1_000,
    gcTime: 0,
  })
  // A new container → start over from its own tail.
  watch(
    () => [toValue(host), toValue(id)] as const,
    () => {
      lines.value = []
      truncated.value = false
    },
  )
  return { lines, truncated, loading: computed(() => tailQ.isLoading.value), following: computed(() => followQ.isFetching.value), refetch: () => tailQ.refetch() }
}

export function useContainerStats(
  host: MaybeRefOrGetter<string>,
  id: MaybeRefOrGetter<string | null>,
  startNs: MaybeRefOrGetter<string>,
  endNs: MaybeRefOrGetter<string>,
) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => ['docker', 'stats', toValue(host), toValue(id) ?? '', String(toValue(startNs)), String(toValue(endNs))]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<ContainerStats> => api.containerStats(toValue(host), toValue(id)!, toValue(startNs), toValue(endNs), { signal }),
    enabled: computed(() => !!toValue(host) && !!toValue(id)),
    refetchInterval: 15_000,
    placeholderData: keepPreviousData,
  })
}

export function useDockerEvents(host: MaybeRefOrGetter<string>, limit = 50) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => ['docker', 'events', toValue(host), limit]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<DockerEventsResult> => api.dockerEvents(toValue(host), limit, { signal }),
    enabled: computed(() => !!toValue(host)),
    refetchInterval: 15_000,
    placeholderData: keepPreviousData,
  })
}

// --- One-shot actions (lifecycle) --------------------------------------------------------------

export const actionsKey = (host: string): string[] => ['docker', 'actions', host]

// Recent actions on a host. Polls fast (2s) while any is queued or running — that is how a row's
// "Restarting…" chip turns into "Restarted" — and every 15s otherwise.
export function useHostActions(host: MaybeRefOrGetter<string>, limit = 20) {
  const qc = useQueryClient()
  const q = useQuery({
    retry: deployRetry,
    queryKey: computed(() => [...actionsKey(toValue(host)), limit]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<ActionsResult> => api.hostActions(toValue(host), limit, { signal }),
    enabled: computed(() => !!toValue(host)),
    refetchInterval: (query) => {
      const rows = (query.state.data as ActionsResult | undefined)?.actions ?? []
      return rows.some((a) => a.status === 'queued' || a.status === 'running') ? 2_000 : 15_000
    },
    placeholderData: keepPreviousData,
  })
  // An action that just settled changed the inventory (and the audit log); refresh both once and
  // say so. "Just settled" means: settled now, and seen in flight by THIS page — the first load and
  // a host switch carry history, and history is not news.
  const seenInFlight = new Set<string>()
  const settledSeen = new Set<string>()
  watch(
    () => [toValue(host), (q.data.value?.actions ?? []).map((a) => `${a.id}:${a.status}`).join(',')] as const,
    ([hostNow, sig], prev) => {
      const rows = q.data.value?.actions ?? []
      if (!prev || prev[0] !== hostNow) {
        // First load for this host: everything present is history.
        seenInFlight.clear()
        settledSeen.clear()
        for (const a of rows) settledSeen.add(a.id)
        for (const a of rows) if (a.status === 'queued' || a.status === 'running') seenInFlight.add(a.id)
        return
      }
      if (sig === prev[1]) return
      let changed = false
      for (const a of rows) {
        if (a.status === 'queued' || a.status === 'running') {
          seenInFlight.add(a.id)
          continue
        }
        if (!settledSeen.has(a.id)) {
          settledSeen.add(a.id)
          if (seenInFlight.has(a.id)) {
            changed = true
            const meta = ACTION_META[a.kind]
            const what = a.container_name ?? a.image ?? toValue(host)
            if (a.status === 'done') toast({ variant: 'success', title: `${meta.done} ${what}`, description: a.result ?? undefined })
            else if (a.status === 'failed') toast({ variant: 'error', title: `${meta.label} failed on ${what}`, description: a.result ?? undefined })
            else toast({ variant: 'warning', title: `${meta.label} on ${what} expired`, description: 'The host did not pick it up in time. Queue it again when the agent is back.' })
          }
        }
      }
      if (changed) {
        qc.invalidateQueries({ queryKey: inventoryKey(toValue(host)) })
        qc.invalidateQueries({ queryKey: auditKey() })
        qc.invalidateQueries({ queryKey: ['docker', 'prune', toValue(host)] })
      }
    },
  )
  return q
}

export function useCreateAction(host: MaybeRefOrGetter<string>) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: ActionInput): Promise<ActionResult> => api.createAction(toValue(host), input),
    onSuccess: (res: ActionResult, input: ActionInput) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: `Couldn't ${ACTION_META[input.kind].label.toLowerCase()}`, description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: actionsKey(toValue(host)) })
      qc.invalidateQueries({ queryKey: auditKey() })
      const a = res.action
      if (a?.status === 'queued') {
        toast({ title: `${ACTION_META[input.kind].label} queued for ${a.container_name ?? a.image ?? toValue(host)}`, description: 'The host is offline. It runs on the next sync, or expires in 15 minutes.' })
      } else {
        toast({ title: `${ACTION_META[input.kind].progressive} ${a?.container_name ?? a?.image ?? toValue(host)}…` })
      }
    },
  })
}

export function usePrunePreview(host: MaybeRefOrGetter<string>, enabled: MaybeRefOrGetter<boolean>) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => ['docker', 'prune', toValue(host)]),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<PrunePreview> => api.prunePreview(toValue(host), { signal }),
    enabled: computed(() => !!toValue(host) && !!toValue(enabled)),
  })
}

// --- Stacks -------------------------------------------------------------------------------------

export const stacksKey = (): string[] => ['stacks']
export const stackKey = (id: string): string[] => ['stacks', 'one', id]

// Fast only while something is transient. `pending` is NOT transient — a stack on an offline
// host is pending for as long as the host is off, which can be a week — so it polls at the
// ordinary cadence; the agent's return shows up within one interval.
function stacksPollMs(rows: Stack[] | undefined): number {
  return (rows ?? []).some((s) => s.status === 'applying' || s.status === 'destroying') ? 2_000 : 15_000
}

// The federation tenant lens (`context.ts`) narrows stacks and rollouts exactly like every other
// signal — here, once, so no page can silently ignore the picker. Applied client-side in `select`
// until the backend takes `?tenant=`; the cache holds the whole fleet either way.
export function useStacks() {
  return useQuery({
    retry: deployRetry,
    queryKey: stacksKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<StacksResult> => api.stacks({ signal }),
    select: (res: StacksResult): StacksResult => (activeTenant.value ? { stacks: res.stacks.filter((s) => s.site === activeTenant.value) } : res),
    refetchInterval: (query) => stacksPollMs((query.state.data as StacksResult | undefined)?.stacks),
    placeholderData: keepPreviousData,
  })
}

export function useStack(id: MaybeRefOrGetter<string>) {
  const qc = useQueryClient()
  const q = useQuery({
    retry: deployRetry,
    queryKey: computed(() => stackKey(toValue(id))),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<Stack | null> => api.stack(toValue(id), { signal }),
    enabled: computed(() => !!toValue(id)),
    refetchInterval: (query) => {
      const s = query.state.data as Stack | null | undefined
      return s ? stacksPollMs([s]) : 15_000
    },
    placeholderData: keepPreviousData,
  })
  // An apply that just settled changed the audit log, the host's containers and the list.
  watch(
    () => q.data.value?.status,
    (status, prev) => {
      if (prev && prev !== status && (status === 'running' || status === 'failed' || status === 'gone' || status === 'declared')) {
        qc.invalidateQueries({ queryKey: stacksKey() })
        qc.invalidateQueries({ queryKey: auditKey() })
        if (q.data.value) qc.invalidateQueries({ queryKey: inventoryKey(q.data.value.host) })
        const s = q.data.value
        if (s && status === 'running') toast({ variant: 'success', title: `${s.name} is running on ${s.host}`, description: `generation ${s.applied_generation} applied` })
        if (s && status === 'failed') toast({ variant: 'error', title: `${s.name} failed on ${s.host}`, description: s.last_error ?? undefined })
        if (s && status === 'gone') toast({ title: `${s.name} destroyed on ${s.host}` })
        if (s && status === 'declared') toast({ title: `${s.name} is down on ${s.host}` })
      }
    },
  )
  return q
}

function stackToast(res: StackResult, title: string, offlineTitle: string): void {
  if (res && res.ok === false) {
    toast({ variant: 'error', title: "Couldn't do that", description: res.error ?? 'Please try again.' })
    return
  }
  const s = res.stack
  if (s && s.status === 'pending') toast({ title: offlineTitle, description: 'The host is offline. It applies on the agent’s next sync.' })
  else toast({ title })
}

export function useCreateStack() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: StackInput): Promise<StackResult> => api.createStack(input),
    onSuccess: (res: StackResult, input: StackInput) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't create the stack", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: stacksKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      const s = res.stack
      if (input.apply === false) toast({ variant: 'success', title: `${input.name} saved`, description: 'Rendered, not applied. Apply it from the stack page.' })
      else if (s && s.status === 'pending' && s.host) toast({ title: `${input.name} queued for ${input.host}`, description: 'Applies on the agent’s next sync.' })
      else toast({ title: `Deploying ${input.name} to ${input.host}…` })
    },
  })
}

export function useUpdateStackFiles() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, files, acknowledged, deployOptions }: { id: string; files: StackFile[]; acknowledged: string[]; deployOptions?: DeployOptions }): Promise<StackResult> =>
      api.updateStackFiles(id, files, acknowledged, deployOptions),
    onSuccess: (res: StackResult, { id }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't save the files", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: stackKey(id) })
      qc.invalidateQueries({ queryKey: stacksKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      toast({ variant: 'success', title: 'Saved', description: res.stack && res.stack.applied_generation !== res.stack.desired_generation ? `Generation ${res.stack.desired_generation} is rendered — apply it when ready.` : undefined })
    },
  })
}

export function useStackAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, kind, args }: { id: string; kind: StackActionKind; args?: StackActionArgs }): Promise<StackResult> => api.stackAction(id, kind, args ?? {}),
    onSuccess: (res: StackResult, { id, kind }) => {
      qc.invalidateQueries({ queryKey: stackKey(id) })
      qc.invalidateQueries({ queryKey: stacksKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      const verb: Record<StackActionKind, string> = { up: 'Applying', down: 'Taking down', restart: 'Restarting', redeploy: 'Redeploying', destroy: 'Destroying', reconcile: 'Reconciling', accept: 'Accepting drift on' }
      stackToast(res, `${verb[kind]} ${res.stack?.name ?? 'stack'}…`, `${verb[kind]} ${res.stack?.name ?? 'stack'} — queued`)
    },
  })
}

export function useValidateCompose() {
  return useMutation({
    mutationFn: (files: StackFile[]): Promise<StackValidation> => api.validateCompose(files),
  })
}

export function useDiscoveredProjects(host: MaybeRefOrGetter<string | null>) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => ['stacks', 'discover', toValue(host) ?? '']),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<DiscoveredProjectsResult> => api.discoverProjects(toValue(host)!, { signal }),
    enabled: computed(() => !!toValue(host)),
  })
}

export function useAdoptProject() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ host, project }: { host: string; project: string }): Promise<StackResult> => api.adoptProject(host, project),
    onSuccess: (res: StackResult, { project, host }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't adopt the project", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: stacksKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      qc.invalidateQueries({ queryKey: ['stacks', 'discover', host] })
      toast({ variant: 'success', title: `${project} is now managed`, description: 'Its containers were not recreated.' })
    },
  })
}

// --- Templates & registries --------------------------------------------------------------------

export const templatesKey = (): string[] => ['templates']
export const templateKey = (id: string): string[] => ['templates', 'one', id]
export const templateVersionKey = (id: string): string[] => ['templates', 'version', id]
export const registriesKey = (): string[] => ['registries']

export function useTemplates() {
  return useQuery({
    retry: deployRetry,
    queryKey: templatesKey(), queryFn: ({ signal }: { signal: AbortSignal }): Promise<TemplatesResult> => api.templates({ signal }), refetchInterval: 30_000 })
}
export function useTemplate(id: MaybeRefOrGetter<string>) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => templateKey(toValue(id))),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<Template | null> => api.template(toValue(id), { signal }),
    enabled: computed(() => !!toValue(id)),
    placeholderData: keepPreviousData,
  })
}
export function useTemplateVersion(id: MaybeRefOrGetter<string | null>) {
  return useQuery({
    retry: deployRetry,
    queryKey: computed(() => templateVersionKey(toValue(id) ?? '')),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<TemplateVersion | null> => api.templateVersion(toValue(id)!, { signal }),
    enabled: computed(() => !!toValue(id)),
    // Versions are immutable — never stale.
    staleTime: Infinity,
  })
}
export function usePublishTemplateVersion() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: TemplateVersionInput): Promise<TemplateVersionResult> => api.publishTemplateVersion(input),
    onSuccess: (res: TemplateVersionResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't publish", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: templatesKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      toast({ variant: 'success', title: `${res.template?.name ?? 'Template'} ${res.version?.version ?? ''} published`, description: 'Immutable from here on; the next change is a new version.' })
    },
  })
}
export function useRegistries() {
  return useQuery({
    retry: deployRetry,
    queryKey: registriesKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RegistriesResult> => api.registries({ signal }),
    refetchInterval: (query) => ((query.state.data as RegistriesResult | undefined)?.registries.some((r) => r.status === 'syncing') ? 1_000 : 30_000),
  })
}
function registryMutation<T>(fn: (v: T) => Promise<RegistryResult | MutationResult>, okTitle: string, errTitle: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: errTitle, description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: registriesKey() })
      qc.invalidateQueries({ queryKey: templatesKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      toast({ variant: 'success', title: okTitle })
    },
  })
}
export const useAddRegistry = () => registryMutation((input: RegistryInput) => api.addRegistry(input), 'Registry added — syncing', "Couldn't add the registry")
export const useSyncRegistry = () => registryMutation((id: string) => api.syncRegistry(id), 'Sync started', "Couldn't sync")
export const useDeleteRegistry = () => registryMutation((id: string) => api.deleteRegistry(id), 'Registry removed', "Couldn't remove the registry")

// --- Image registry credentials (private pulls) -------------------------------------------------

export const imageRegistriesKey = (): string[] => ['image-registries']
export function useImageRegistries() {
  return useQuery({
    retry: deployRetry,
    queryKey: imageRegistriesKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RegistryCredentialsResult> => api.imageRegistries({ signal }),
    staleTime: 30_000,
  })
}
function imageRegistryMutation<T>(fn: (v: T) => Promise<RegistryCredentialResult | MutationResult>, okTitle: string, errTitle: string) {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: errTitle, description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: imageRegistriesKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      toast({ variant: 'success', title: okTitle })
    },
  })
}
export const useAddImageRegistry = () => imageRegistryMutation((input: RegistryCredentialInput) => api.addImageRegistry(input), 'Registry credential stored', "Couldn't store the credential")
export const useUpdateImageRegistry = () => imageRegistryMutation(({ id, input }: { id: string; input: RegistryCredentialInput }) => api.updateImageRegistry(id, input), 'Registry credential updated', "Couldn't update the credential")
export const useDeleteImageRegistry = () => imageRegistryMutation((id: string) => api.deleteImageRegistry(id), 'Registry credential removed', "Couldn't remove the credential")
export function useImportPortainer() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ json, source }: { json: string; source: string }): Promise<ImportResult> => api.importPortainer(json, source),
    onSuccess: (res: ImportResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't import", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: templatesKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      toast({ variant: 'success', title: `Imported ${res.imported ?? 0} templates`, description: res.skipped ? `${res.skipped} skipped (Swarm or unsupported types).` : undefined })
    },
  })
}
export function useDryRun() {
  return useMutation({ mutationFn: (input: TemplateDeployInput): Promise<DryRunResult> => api.dryRun(input) })
}
export function useDeployTemplate() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: TemplateDeployInput): Promise<StackResult> => api.deployTemplate(input),
    onSuccess: (res: StackResult, input: TemplateDeployInput) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't deploy", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: stacksKey() })
      qc.invalidateQueries({ queryKey: templatesKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      if (res.stack) qc.invalidateQueries({ queryKey: stackKey(res.stack.id) })
      const s = res.stack
      if (s && s.status === 'pending' && s.host) toast({ title: `${input.name} queued for ${input.host}`, description: 'Applies on the agent’s next sync.' })
      else toast({ title: `Deploying ${input.name} to ${input.host}…` })
    },
  })
}

// --- Rollouts ------------------------------------------------------------------------------------

export const rolloutsKey = (): string[] => ['rollouts']
export const rolloutKey = (id: string): string[] => ['rollouts', 'one', id]

// Fast only while a target is actually moving (pre-pulling / applying). A running rollout whose
// stage is waiting on an offline host is not transient — it can sit there for a week.
function rolloutPollMs(rows: Rollout[] | undefined): number {
  return (rows ?? []).some((r) => r.state === 'running' && r.targets.some((t) => t.status === 'prepulling' || t.status === 'applying')) ? 2_000 : 15_000
}

export function useRollouts() {
  return useQuery({
    retry: deployRetry,
    queryKey: rolloutsKey(),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<RolloutsResult> => api.rollouts({ signal }),
    select: (res: RolloutsResult): RolloutsResult => (activeTenant.value ? { rollouts: res.rollouts.filter((r) => r.targets.some((t) => t.site === activeTenant.value)) } : res),
    refetchInterval: (query) => rolloutPollMs((query.state.data as RolloutsResult | undefined)?.rollouts),
    placeholderData: keepPreviousData,
  })
}

export function useRollout(id: MaybeRefOrGetter<string>) {
  const qc = useQueryClient()
  const q = useQuery({
    retry: deployRetry,
    queryKey: computed(() => rolloutKey(toValue(id))),
    queryFn: ({ signal }: { signal: AbortSignal }): Promise<Rollout | null> => api.rollout(toValue(id), { signal }),
    enabled: computed(() => !!toValue(id)),
    refetchInterval: (query) => {
      const r = query.state.data as Rollout | null | undefined
      return r ? rolloutPollMs([r]) : 15_000
    },
    placeholderData: keepPreviousData,
  })
  // A state change is news: refresh the list, the stacks and the audit log, and say it once.
  watch(
    () => q.data.value?.state,
    (state, prev) => {
      const r = q.data.value
      if (!r || !prev || prev === state) return
      qc.invalidateQueries({ queryKey: rolloutsKey() })
      qc.invalidateQueries({ queryKey: stacksKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      if (state === 'paused_gate') toast({ variant: 'warning', title: `${r.name}: canary done`, description: 'Waiting at the gate. Promote when you are satisfied.' })
      if (state === 'paused_threshold') toast({ variant: 'error', title: `${r.name}: paused`, description: 'Too many failures in this stage. Nothing further is applied.' })
      if (state === 'completed') toast({ variant: 'success', title: `${r.name}: completed` })
    },
  )
  return q
}

export function useCreateRollout() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: RolloutInput): Promise<RolloutResult> => api.createRollout(input),
    onSuccess: (res: RolloutResult, input: RolloutInput) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't create the rollout", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: rolloutsKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      toast({ variant: 'success', title: input.start ? 'Rollout started' : 'Rollout planned', description: input.start ? 'The canary stage is applying.' : 'Start it from its page when ready.' })
    },
  })
}

export function useRolloutAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, kind }: { id: string; kind: RolloutActionKind }): Promise<RolloutResult> => api.rolloutAction(id, kind),
    onSuccess: (res: RolloutResult, { id, kind }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't do that", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: rolloutKey(id) })
      qc.invalidateQueries({ queryKey: rolloutsKey() })
      qc.invalidateQueries({ queryKey: stacksKey() })
      qc.invalidateQueries({ queryKey: auditKey() })
      const VERB: Record<RolloutActionKind, string> = { start: 'Started', promote: 'Promoted to the next stage', pause: 'Paused', resume: 'Resumed', abort: 'Aborted', rollback: 'Rolling back', retry: 'Retrying failed targets' }
      toast({ title: `${VERB[kind]}: ${res.rollout?.name ?? 'rollout'}` })
    },
  })
}

// --- Deploy impact: the RED rows before and after an apply -------------------------------------

import { useServicesList } from '@/lib/services/servicesQueries'
import { judgeImpact, impactWindows, type ImpactSummary } from '@/lib/deploy/impact'

const toNs = (ms: number): string => (BigInt(Math.round(ms)) * 1_000_000n).toString()

/**
 * "Did it work?" for a set of APM services around an apply moment: 15 minutes before against the
 * 15 minutes after (or until now), judged by `judgeImpact`. Reuses the services RED query, so the
 * numbers are exactly what the Backend world shows for the same window.
 */
export function useDeployImpact(services: MaybeRefOrGetter<string[]>, appliedMs: MaybeRefOrGetter<number | null>, windowMs = 15 * 60_000) {
  const windows = computed(() => {
    const at = toValue(appliedMs)
    return at ? impactWindows(at, Date.now(), windowMs) : null
  })
  const enabled = computed(() => !!windows.value && toValue(services).length > 0)
  const before = useServicesList(() => '', () => (windows.value ? toNs(windows.value.before[0]) : '0'), () => (windows.value ? toNs(windows.value.before[1]) : '1'))
  const after = useServicesList(() => '', () => (windows.value ? toNs(windows.value.after[0]) : '0'), () => (windows.value ? toNs(windows.value.after[1]) : '1'))
  const summary = computed<ImpactSummary | null>(() => (enabled.value ? judgeImpact(toValue(services), before.data.value ?? [], after.data.value ?? []) : null))
  const loading = computed(() => enabled.value && (before.isLoading.value || after.isLoading.value))
  return { summary, loading, windows, enabled }
}
