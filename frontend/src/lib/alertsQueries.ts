// Thin TanStack Query wrappers over the `api.*` alert-engine methods (webhook alert & notification
// rules/channels/incidents — cross-signal, so this lives at `lib/` root rather than under a single
// signal's subfolder like `lib/uptime/` or `lib/rum/`). Same contract as `uptimeQueries.ts`: reactive
// inputs (refs OR getter functions, normalized with `toValue`), a reactive `queryKey` for
// refetch-on-change + dedup, a threaded AbortSignal for cancellation, and `useRules`/`useChannels`/
// `useIncidents` all poll every ~15s like `useMonitors` (design doc §11). Every mutation's
// `api.*` method already returns the non-throwing `{ ok, error }` shape (mirrors
// `useCreateRumApp`/`useSetRetention`), so `onSuccess` is the single place that both invalidates the
// cache and toasts, branching on the resolved result — mutations here never reject.
import { computed, toValue } from 'vue'
import type { MaybeRefOrGetter } from 'vue'
import { useQuery, useMutation, useQueryClient } from '@tanstack/vue-query'
import {
  api,
  type AlertRule,
  type AlertRuleInput,
  type AlertRuleResult,
  type AlertChannel,
  type AlertChannelInput,
  type AlertChannelResult,
  type AlertIncident,
  type AlertIncidentsFilter,
  type AlertCondition,
  type AlertPreviewResult,
  type AlertTestRuleResult,
  type MutationResult,
} from '@/lib/core/api'
import { toast } from '@/components/ui/toast'

// --- Query keys --------------------------------------------------------------------------------

export const alertRulesQueryKey = (): string[] => ['alerts', 'rules']
export const alertChannelsQueryKey = (): string[] => ['alerts', 'channels']
export const alertIncidentsQueryKey = (
  filters: MaybeRefOrGetter<AlertIncidentsFilter>,
): (string | number)[] => {
  const f = toValue(filters) ?? {}
  return ['alerts', 'incidents', f.status ?? '', f.rule_id ?? '', f.limit ?? '']
}
export const alertPreviewQueryKey = (condition: MaybeRefOrGetter<AlertCondition | null | undefined>): string[] => [
  'alerts',
  'preview',
  JSON.stringify(toValue(condition) ?? null),
]

// --- Queries -------------------------------------------------------------------------------------

// All rules — polled for live status pills (triggered/pending/ok/paused derive from this + incidents).
export function useRules() {
  return useQuery({
    queryKey: alertRulesQueryKey(),
    queryFn: ({ signal }): Promise<AlertRule[]> => api.alertRules({ signal }),
    refetchInterval: 15_000,
  })
}

// All notification channels — polled so a channel's last-delivery health stays fresh.
export function useChannels() {
  return useQuery({
    queryKey: alertChannelsQueryKey(),
    queryFn: ({ signal }): Promise<AlertChannel[]> => api.alertChannels({ signal }),
    refetchInterval: 15_000,
  })
}

// Incident history (currently-triggered + resolved), optionally filtered by status/rule/limit.
export function useIncidents(filters: MaybeRefOrGetter<AlertIncidentsFilter> = {}) {
  return useQuery({
    queryKey: computed(() => alertIncidentsQueryKey(filters)),
    queryFn: ({ signal }): Promise<AlertIncident[]> => api.alertIncidents(toValue(filters) ?? {}, { signal }),
    refetchInterval: 15_000,
  })
}

// Dry-run a draft condition → current series+values, powering the create/edit dialog's live
// "will trigger on N series now" preview. Disabled while there's no condition to preview yet
// (e.g. the signal step of the dialog hasn't been filled in).
export function usePreview(condition: MaybeRefOrGetter<AlertCondition | null | undefined>) {
  return useQuery({
    queryKey: computed(() => alertPreviewQueryKey(condition)),
    queryFn: ({ signal }): Promise<AlertPreviewResult> => api.alertPreview(toValue(condition) as AlertCondition, { signal }),
    enabled: computed(() => toValue(condition) != null),
  })
}

// --- Rule mutations --------------------------------------------------------------------------

export function useCreateRule() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: AlertRuleInput): Promise<AlertRuleResult> => api.createAlertRule(input),
    onSuccess: (res: AlertRuleResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't create rule", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: alertRulesQueryKey() })
      toast({ variant: 'success', title: 'Rule created' })
    },
  })
}

export function useUpdateRule() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: AlertRuleInput }): Promise<AlertRuleResult> =>
      api.updateAlertRule(id, input),
    onSuccess: (res: AlertRuleResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't update rule", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: alertRulesQueryKey() })
      toast({ variant: 'success', title: 'Rule updated' })
    },
  })
}

export function useDeleteRule() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string): Promise<MutationResult> => api.deleteAlertRule(id),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't delete rule", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: alertRulesQueryKey() })
      toast({ variant: 'success', title: 'Rule deleted' })
    },
  })
}

// The enable toggle IS the v1 mute (design doc §2) — no separate "pause" endpoint, just a PATCH
// of `enabled`.
export function useToggleRule() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }): Promise<AlertRuleResult> =>
      api.toggleAlertRule(id, enabled),
    onSuccess: (res: AlertRuleResult, { enabled }: { id: string; enabled: boolean }) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't update rule", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: alertRulesQueryKey() })
      toast({ variant: 'success', title: enabled ? 'Rule enabled' : 'Rule paused' })
    },
  })
}

// Evaluate a saved rule right now and report which series would fire — doesn't touch state.
export function useTestRule() {
  return useMutation({
    mutationFn: (id: string): Promise<AlertTestRuleResult> => api.testAlertRule(id),
    onSuccess: (res: AlertTestRuleResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't test rule", description: res.error ?? 'Please try again.' })
        return
      }
      const n = res.series?.length ?? 0
      toast({ variant: 'success', title: n ? `Would trigger on ${n} series` : 'No series would trigger now' })
    },
  })
}

// --- Channel mutations -----------------------------------------------------------------------

export function useCreateChannel() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (input: AlertChannelInput): Promise<AlertChannelResult> => api.createAlertChannel(input),
    onSuccess: (res: AlertChannelResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't add channel", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: alertChannelsQueryKey() })
      toast({ variant: 'success', title: 'Channel added' })
    },
  })
}

export function useUpdateChannel() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: AlertChannelInput }): Promise<AlertChannelResult> =>
      api.updateAlertChannel(id, input),
    onSuccess: (res: AlertChannelResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't save channel", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: alertChannelsQueryKey() })
      toast({ variant: 'success', title: 'Channel updated' })
    },
  })
}

export function useDeleteChannel() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string): Promise<MutationResult> => api.deleteAlertChannel(id),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't remove channel", description: res.error ?? 'Please try again.' })
        return
      }
      qc.invalidateQueries({ queryKey: alertChannelsQueryKey() })
      toast({ variant: 'success', title: 'Channel removed' })
    },
  })
}

// Sends a sample webhook to this channel right now — doesn't invalidate anything.
export function useTestChannel() {
  return useMutation({
    mutationFn: (id: string): Promise<MutationResult> => api.testAlertChannel(id),
    onSuccess: (res: MutationResult) => {
      if (res && res.ok === false) {
        toast({ variant: 'error', title: "Couldn't send test webhook", description: res.error ?? 'Please try again.' })
        return
      }
      toast({ variant: 'success', title: 'Test webhook sent' })
    },
  })
}

// Sends a sample delivery for an unsaved channel draft — powers the ChannelDialog's in-form Test
// button so the user can verify a preset before it's ever persisted.
export function useTestChannelDraft() {
  return useMutation({
    mutationFn: (input: AlertChannelInput): Promise<MutationResult> => api.testAlertChannelDraft(input),
  })
}
