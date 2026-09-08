import { describe, it, expect } from 'vitest'
import { installSnippet, orderedCapabilities, upgradeAvailable, AGENT_STATUS_META } from './agentStatus'

describe('agentStatus', () => {
  it('orders capabilities canonically whatever the agent reported', () => {
    expect(orderedCapabilities(['exec', 'read', 'apply'])).toEqual(['read', 'apply', 'exec'])
  })

  it('flags an upgrade only when both versions are known and differ', () => {
    expect(upgradeAvailable({ agent_version: '1.7.2', latest_agent_version: '1.8.0' })).toBe(true)
    expect(upgradeAvailable({ agent_version: '1.8.0', latest_agent_version: '1.8.0' })).toBe(false)
    expect(upgradeAvailable({ agent_version: null, latest_agent_version: '1.8.0' })).toBe(false)
  })

  it('offline is the only error-toned state; waiting is neutral', () => {
    expect(AGENT_STATUS_META.offline.tone).toBe('error')
    expect(AGENT_STATUS_META.pending.tone).toBe('neutral')
    expect(AGENT_STATUS_META.online.tone).toBe('success')
  })

  it('builds an install snippet that downloads from THIS Photon and carries the capability flags', () => {
    const s = installSnippet({ token: 'ak_x', host: 'edge-1', capabilities: ['lifecycle', 'read'], pollSecs: 30, origin: 'https://photon.local' }, 'systemd')
    expect(s).toContain('https://photon.local/api/agent/download?arch=')
    expect(s).toContain('--token ak_x')
    expect(s).toContain('--docker-read --docker-lifecycle')
    expect(s).toContain('--poll 30s')
    const d = installSnippet({ token: 'ak_x', host: 'edge-1', capabilities: ['read'], pollSecs: 15, origin: 'https://photon.local' }, 'docker')
    expect(d).toContain('docker run')
    expect(d).toContain('PHOTON_AGENT_TOKEN=ak_x')
  })
})
