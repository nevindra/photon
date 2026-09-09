import { describe, it, expect } from 'vitest'
import { generationLabel, hasPendingGeneration, sourceLabel, driftSummary, stackCounts, filterStacks, sortStacks, groupStacksByHost, lastChangeLabel, STACK_STATUS_META } from './stacks'
import type { Stack } from '@/lib/core/api'

function st(over: Partial<Stack>): Stack {
  return {
    id: 'x', name: 'n', host: 'h', site: null, source: 'compose', template_id: null, template_name: null, template_version: null, status: 'running',
    desired_generation: 1, applied_generation: 1, files: [], variables: {}, secrets: [], deploy_options: { pull: 'missing', force_recreate: false, recreate_volumes: false },
    services: [], drift: [], apm_services: [], acknowledged_warnings: [], last_error: null, created_by: 'r', created_ms: 0, updated_ms: 0, last_applied_ms: null,
    ...over,
  }
}

describe('stacks vocabulary', () => {
  it('pending is neutral, never red', () => {
    expect(STACK_STATUS_META.pending.tone).toBe('neutral')
    expect(STACK_STATUS_META.failed.tone).toBe('error')
    expect(STACK_STATUS_META.drifted.attention).toBe(true)
  })

  it('shows the generation gap when a newer one waits', () => {
    expect(generationLabel({ applied_generation: 15, desired_generation: 15 })).toBe('15')
    expect(generationLabel({ applied_generation: 14, desired_generation: 15 })).toBe('14 → 15')
    expect(generationLabel({ applied_generation: null, desired_generation: 1 })).toBe('→ 1')
    expect(hasPendingGeneration({ applied_generation: 14, desired_generation: 15 })).toBe(true)
  })

  it('names the source and summarises drift', () => {
    expect(sourceLabel(st({ source: 'template', template_name: 'vas', template_version: '2.4.1' }))).toBe('vas 2.4.1')
    expect(sourceLabel(st({ source: 'adopted' }))).toBe('adopted')
    expect(driftSummary([])).toBe('no drift')
    expect(driftSummary([{ kind: 'image_changed', service: 'worker', field: 'image', desired: 'a', observed: 'b', since_ms: 0 }])).toBe('1 change: image changed (worker)')
  })

  it('counts, filters and sorts attention first', () => {
    const rows = [st({ id: 'a', name: 'b', status: 'running' }), st({ id: 'b', name: 'a', status: 'failed' }), st({ id: 'c', name: 'c', status: 'drifted', host: 'edge' }), st({ id: 'd', name: 'd', status: 'gone' })]
    expect(stackCounts(rows)).toEqual({ total: 3, running: 1, drifted: 1, failed: 1, pending: 0 })
    expect(filterStacks(rows, {}).map((s) => s.id)).toEqual(['a', 'b', 'c'])
    expect(filterStacks(rows, { attention: true }).map((s) => s.id)).toEqual(['b', 'c'])
    expect(filterStacks(rows, { host: 'edge' }).map((s) => s.id)).toEqual(['c'])
    expect(filterStacks(rows, { q: 'B' }).map((s) => s.id)).toEqual(['a'])
    expect(sortStacks(rows).map((s) => s.id)).toEqual(['b', 'c', 'a', 'd'])
  })
})

describe('the host-grouped list', () => {
  it('groups sorted stacks under hosts, a host placed by its worst stack', () => {
    const rows = sortStacks([
      st({ id: 'a', name: 'alpha', host: 'web-1', status: 'running' }),
      st({ id: 'b', name: 'beta', host: 'edge-1', status: 'running' }),
      st({ id: 'c', name: 'gamma', host: 'edge-1', status: 'failed' }),
      st({ id: 'd', name: 'delta', host: 'web-1', site: 'jakarta', status: 'pending' }),
    ])
    const groups = groupStacksByHost(rows)
    expect(groups.map((g) => g.host)).toEqual(['edge-1', 'web-1'])
    expect(groups[0].stacks.map((s) => s.id)).toEqual(['c', 'b'])
    expect(groups[1].stacks.map((s) => s.id)).toEqual(['d', 'a'])
    expect(groups[1].site).toBe('jakarta')
  })

  it('filters by the chip strip bucket', () => {
    const all = [st({ id: 'r', status: 'running' }), st({ id: 'p', status: 'pending' }), st({ id: 'a', status: 'applying' }), st({ id: 'f', status: 'failed' })]
    expect(filterStacks(all, { bucket: 'running' }).map((s) => s.id)).toEqual(['r'])
    expect(filterStacks(all, { bucket: 'pending' }).map((s) => s.id)).toEqual(['p', 'a'])
    expect(filterStacks(all, { bucket: null }).length).toBe(4)
  })

  it('says what happened last in the status\'s own verb', () => {
    const now = 1_000_000_000
    expect(lastChangeLabel(st({ status: 'failed', updated_ms: now - 2 * 3600_000 }), now)).toBe('apply failed 2h ago')
    expect(lastChangeLabel(st({ status: 'running', updated_ms: now - 60_000, last_applied_ms: now - 55 * 60_000 }), now)).toBe('applied 55m ago')
    expect(lastChangeLabel(st({ status: 'pending', updated_ms: now - 5 * 60_000, created_by: 'alfin' }), now)).toBe('saved 5m ago by alfin')
    expect(lastChangeLabel(st({ status: 'drifted', updated_ms: now - 14 * 60_000 }), now)).toBe('drift seen 14m ago')
  })
})
