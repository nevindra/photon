import { describe, it, expect } from 'vitest'
import { installSnippet, originIssue, orderedCapabilities, upgradeAvailable, AGENT_STATUS_META } from './agentStatus'

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

  it('builds an install snippet that downloads from THIS Photon, runs privileged steps with sudo and carries the flags', () => {
    const s = installSnippet({ token: 'ak_x', host: 'edge-1', capabilities: ['lifecycle', 'read'], pollSecs: 30, origin: 'https://photon.local/' }, 'systemd')
    expect(s).toContain('sudo curl -fSsL -o /usr/local/bin/photon-agent "https://photon.local/api/agent/download?arch=')
    expect(s).toContain('sudo chmod +x /usr/local/bin/photon-agent')
    expect(s).toContain('sudo photon-agent install --server https://photon.local --host-name edge-1')
    expect(s).toContain('--docker-read --docker-lifecycle --poll 30s')
    expect(s).toContain('systemctl status photon-agent')
  })

  it('keeps the token out of the command unless asked, and says so either way', () => {
    const prompted = installSnippet({ token: 'ak_x', host: 'edge-1', capabilities: ['read'], pollSecs: 15, origin: 'https://photon.local' }, 'systemd')
    expect(prompted).not.toContain('ak_x')
    expect(prompted).toContain("you'll be asked for the token")
    const embedded = installSnippet({ token: 'ak_x', host: 'edge-1', capabilities: ['read'], pollSecs: 15, origin: 'https://photon.local', embedToken: true }, 'systemd')
    expect(embedded).toContain('--token ak_x')
    expect(embedded).toContain('shell history')
  })

  it('the foreground format needs no root and installs nothing', () => {
    const f = installSnippet({ token: 'ak_x', host: 'edge-1', capabilities: ['read'], pollSecs: 15, origin: 'http://10.0.0.5:8080' }, 'foreground')
    expect(f).not.toContain('sudo')
    expect(f).not.toContain('photon-agent install')
    expect(f).toContain('curl -fSsL -o photon-agent "http://10.0.0.5:8080/api/agent/download?arch=')
    expect(f).toContain('./photon-agent --server http://10.0.0.5:8080 --host-name edge-1')
    expect(f).toContain('--docker-read --poll 15s')
  })

  it('names what is wrong with the URL the host would dial', () => {
    expect(originIssue('http://localhost:5173')).toBe('loopback')
    expect(originIssue('http://127.0.0.1:8080/')).toBe('loopback')
    expect(originIssue('http://10.0.0.5:8080')).toBe('plain-http')
    expect(originIssue('https://photon.example.com')).toBeNull()
    expect(originIssue('photon.example.com')).toBe('invalid')
    expect(originIssue('ftp://x')).toBe('invalid')
  })
})
