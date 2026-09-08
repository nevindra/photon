import { describe, it, expect } from 'vitest'
import { containerTone, groupByProject, formatPort, matchesFilter, inventoryKpis, shortImage, eventTone, eventLabel, eventDetail, portUrl, foldEvents, everyLabel, uniquePorts } from './containers'
import type { Container } from '@/lib/core/api'

function c(over: Partial<Container>): Container {
  return {
    id: 'id', name: 'n', image: 'img:1', image_id: 'sha256:abc', command: '', state: 'running', health: 'none', status_text: '', created_ms: 0, started_ms: 0,
    finished_ms: null, exit_code: null, restart_count: 0, restart_policy: 'no', ports: [], mounts: [], networks: [], labels: {}, env_keys: [], env: {},
    compose_project: null, compose_service: null, managed_stack_id: null, cpu_pct: null, mem_bytes: null, mem_limit_bytes: null, net_rx_bps: null, net_tx_bps: null, block_read_bps: null, block_write_bps: null,
    ...over,
  }
}

describe('containers helpers', () => {
  it('tone is the worse of state and health; a non-zero exit is an error', () => {
    expect(containerTone(c({ state: 'running', health: 'healthy' }))).toBe('success')
    expect(containerTone(c({ state: 'running', health: 'unhealthy' }))).toBe('error')
    expect(containerTone(c({ state: 'exited', exit_code: 0 }))).toBe('neutral')
    expect(containerTone(c({ state: 'exited', exit_code: 137 }))).toBe('error')
    expect(containerTone(c({ state: 'restarting' }))).toBe('error')
    expect(containerTone(c({ state: 'paused' }))).toBe('warning')
  })

  it('groups by compose project, named projects first, standalone last, services sorted', () => {
    const groups = groupByProject([
      c({ name: 'z-standalone' }),
      c({ name: 'p-web-1', compose_project: 'photon', compose_service: 'web', managed_stack_id: 'st1' }),
      c({ name: 'p-api-1', compose_project: 'photon', compose_service: 'api', state: 'exited', exit_code: 1 }),
      c({ name: 'a-x', compose_project: 'alpha', compose_service: 'x' }),
    ])
    expect(groups.map((g) => g.project)).toEqual(['alpha', 'photon', null])
    const photon = groups[1]
    expect(photon.containers.map((x) => x.compose_service)).toEqual(['api', 'web'])
    expect(photon.managedStackId).toBe('st1')
    expect(photon.running).toBe(1)
    expect(photon.total).toBe(2)
    expect(photon.tone).toBe('error')
  })

  it('formats ports the way docker ps does, arrow for published', () => {
    expect(formatPort({ host_ip: '0.0.0.0', host_port: 8080, container_port: 80, protocol: 'tcp' })).toBe('8080→80')
    expect(formatPort({ host_ip: '127.0.0.1', host_port: 5432, container_port: 5432, protocol: 'tcp' })).toBe('127.0.0.1:5432→5432')
    expect(formatPort({ host_ip: null, host_port: null, container_port: 9001, protocol: 'tcp' })).toBe('9001/tcp')
    expect(formatPort({ host_ip: '0.0.0.0', host_port: 53, container_port: 53, protocol: 'udp' })).toBe('53→53/udp')
  })

  it('filters on name, image, project and label values, case-insensitively', () => {
    const x = c({ name: 'vas-api-1', image: 'registry.local/nodeflux/vas-api:2.4.1', compose_project: 'nodeflux-vas', labels: { team: 'Edge' } })
    expect(matchesFilter(x, 'VAS-API')).toBe(true)
    expect(matchesFilter(x, 'nodeflux')).toBe(true)
    expect(matchesFilter(x, 'edge')).toBe(true)
    expect(matchesFilter(x, 'postgres')).toBe(false)
    expect(matchesFilter(x, '  ')).toBe(true)
  })

  it('counts the KPIs', () => {
    const k = inventoryKpis({ containers: [c({ state: 'running', restart_count: 3, compose_project: 'a' }), c({ state: 'exited' }), c({ state: 'running', health: 'unhealthy', compose_project: 'a' }), c({ state: 'restarting', restart_count: 9 })] })
    expect(k).toEqual({ running: 2, total: 4, stopped: 1, unhealthy: 2, restarts: 12, projects: 1 })
  })

  it('shortens registry-qualified images and reads events', () => {
    expect(shortImage('registry.divtik.local/nodeflux/vas-api:2.4.1')).toBe('nodeflux/vas-api:2.4.1')
    expect(shortImage('postgres:16')).toBe('postgres:16')
    expect(shortImage('ghcr.io/nodeflux/api:1')).toBe('nodeflux/api:1')
    expect(eventTone('oom')).toBe('error')
    expect(eventTone('die', { exitCode: '0' })).toBe('neutral')
    expect(eventTone('die', { exitCode: '1' })).toBe('error')
    expect(eventTone('start')).toBe('success')
    expect(eventLabel('die', { exitCode: '137' })).toBe('died (exit 137)')
    expect(eventLabel('health_status: unhealthy')).toBe('health unhealthy')
  })

  it('keeps only the verb of an exec event in the label and hands the command to eventDetail', () => {
    const action = 'exec_start: /bin/sh -c pg_isready -U postgres -d vdashboard'
    expect(eventLabel(action)).toBe('exec_start')
    expect(eventDetail(action)).toBe('/bin/sh -c pg_isready -U postgres -d vdashboard')
    expect(eventDetail('exec_die')).toBeNull()
    expect(eventDetail('health_status: unhealthy')).toBeNull()
  })
})

describe('port links and event folding', () => {
  it('opens a published tcp port on the host, a bound address, or not at all', () => {
    expect(portUrl({ host_ip: '0.0.0.0', host_port: 4030, container_port: 4030, protocol: 'tcp' }, 'dev-laptop')).toBe('http://dev-laptop:4030')
    expect(portUrl({ host_ip: '::', host_port: 443, container_port: 8443, protocol: 'tcp' }, 'edge-1')).toBe('https://edge-1:443')
    expect(portUrl({ host_ip: '192.168.103.76', host_port: 8080, container_port: 80, protocol: 'tcp' }, 'dev-laptop')).toBe('http://192.168.103.76:8080')
    expect(portUrl({ host_ip: '127.0.0.1', host_port: 5432, container_port: 5432, protocol: 'tcp' }, 'db-1')).toBe('http://db-1:5432')
    expect(portUrl({ host_ip: null, host_port: null, container_port: 5432, protocol: 'tcp' }, 'db-1')).toBeNull()
    expect(portUrl({ host_ip: '0.0.0.0', host_port: 53, container_port: 53, protocol: 'udp' }, 'dns')).toBeNull()
  })

  it('folds one actor\'s exec chatter into a single row that keeps its place, count and cadence', () => {
    const t = 1_000_000
    const ev = (id: string, ts: number, action: string, actor = 'lenz-db-1') => ({ id, ts_ms: ts, type: 'container' as const, actor, action, attributes: { image: 'timescale/timescaledb-ha:pg17' } })
    const events = [
      ev('a', t + 20_000, 'exec_die'),
      ev('b', t + 20_000, 'exec_start: /bin/sh -c pg_isready -U postgres -d vdashboard'),
      ev('c', t + 20_000, 'exec_create: /bin/sh -c pg_isready -U postgres -d vdashboard'),
      ev('d', t + 15_000, 'start', 'lenz-lenz-1'),
      ev('e', t + 10_000, 'exec_die'),
      ev('f', t + 10_000, 'exec_start: /bin/sh -c pg_isready -U postgres -d vdashboard'),
      ev('g', t + 10_000, 'exec_create: /bin/sh -c pg_isready -U postgres -d vdashboard'),
      ev('h', t, 'exec_start: /bin/sh -c pg_isready -U postgres -d vdashboard'),
      ev('i', t - 5_000, 'oom', 'worker'),
    ]
    const feed = foldEvents(events)
    expect(feed.map((i) => (i.kind === 'fold' ? `fold:${i.actor}` : i.event.action))).toEqual(['fold:lenz-db-1', 'start', 'oom'])
    const fold = feed[0]
    if (fold.kind !== 'fold') throw new Error('expected a fold')
    expect(fold.count).toBe(7)
    expect(fold.command).toBe('/bin/sh -c pg_isready -U postgres -d vdashboard')
    expect(fold.every_ms).toBe(10_000)
    expect(fold.ts_ms).toBe(t + 20_000)
    expect(everyLabel(fold.every_ms!)).toBe('every 10s')
    expect(everyLabel(120_000)).toBe('every 2m')
  })

  it('collapses the IPv4 / IPv6 twins of one published port', () => {
    const v4 = { host_ip: '0.0.0.0', host_port: 4030, container_port: 4030, protocol: 'tcp' as const }
    const v6 = { host_ip: '::', host_port: 4030, container_port: 4030, protocol: 'tcp' as const }
    const bound = { host_ip: '192.168.103.76', host_port: 4030, container_port: 4030, protocol: 'tcp' as const }
    expect(uniquePorts([v4, v6, bound])).toEqual([v4, bound])
  })

  it('leaves a feed with no exec events untouched', () => {
    const e = { id: 'x', ts_ms: 1, type: 'container' as const, actor: 'a', action: 'die', attributes: {} }
    expect(foldEvents([e])).toEqual([{ kind: 'event', event: e }])
  })
})
