import { describe, it, expect } from 'vitest'
import { ACTION_META, availableActions, primaryAction, pruneSummary } from './actions'

describe('actions', () => {
  it('offers the actions that make sense for the state, primary first', () => {
    expect(primaryAction({ state: 'running' })).toBe('restart')
    expect(primaryAction({ state: 'exited' })).toBe('start')
    expect(primaryAction({ state: 'restarting' })).toBe('stop')
    expect(primaryAction({ state: 'removing' })).toBeNull()
    expect(availableActions({ state: 'running' })).toEqual(['restart', 'stop', 'kill', 'pull', 'recreate', 'remove'])
    expect(availableActions({ state: 'exited' })).toEqual(['start', 'pull', 'recreate', 'remove'])
    expect(availableActions({ state: 'removing' })).toEqual([])
  })

  it('sizes the confirmation to the blast radius', () => {
    expect(ACTION_META.start.risk).toBe('none')
    expect(ACTION_META.restart.risk).toBe('confirm')
    expect(ACTION_META.remove.risk).toBe('typed')
    expect(ACTION_META.remove.destructive).toBe(true)
    expect(ACTION_META.kill.destructive).toBe(true)
    expect(ACTION_META.stop.destructive).toBe(false)
  })

  it('explains a stop in terms of the ports that go dark', () => {
    const text = ACTION_META.stop.explain({ name: 'api', image: 'x', ports: [{ host_ip: null, host_port: 8080, container_port: 8080, protocol: 'tcp' }] })
    expect(text).toContain('SIGTERM')
    expect(text).toContain('8080')
    expect(ACTION_META.stop.explain({ name: 'w', image: 'x', ports: [] })).not.toContain('answering')
  })

  it('summarises a prune from the selected categories only', () => {
    const fmt = (b: number) => `${b}B`
    expect(pruneSummary([{ label: 'Images', bytes: 100, count: 2, selected: true }, { label: 'Volumes', bytes: 50, count: 1, selected: false }], fmt)).toBe('Would free 100B (images 100B)')
    expect(pruneSummary([{ label: 'Images', bytes: 0, count: 0, selected: true }], fmt)).toBe('Nothing selected.')
  })
})
