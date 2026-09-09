import { describe, it, expect } from 'vitest'
import { defaultVariables, validateVariables, groupVariables, renderEnv, semverCompare, nextVersion, manifestToYaml, scopeText, requirementChecks } from './templates'
import type { DockerFacts } from '@/lib/core/api'
import type { TemplateManifest } from '@/lib/core/api'

const M: TemplateManifest = {
  id: 't', version: '1.0.0', name: 'T', description: '', icon: '', categories: [],
  variables: [
    { name: 'VERSION', label: 'Version', type: 'string', required: true, default: '1.0.0', preset: true },
    { name: 'PORT', label: 'Port', type: 'port', required: true, default: '8443', group: 'Network' },
    { name: 'MODEL', label: 'Model', type: 'select', required: true, options: ['a', 'b'] },
    { name: 'DEBUG', label: 'Debug', type: 'bool', required: false },
    { name: 'DATA', label: 'Data path', type: 'path', required: false, default: '/data' },
    { name: 'LICENSE', label: 'License', type: 'secret', required: true },
    { name: 'DB_PASSWORD', label: 'DB', type: 'generated', required: true },
  ],
  requires: { arch: ['x86_64'], min_docker: '24.0' }, ports: [], volumes: [], health: null,
  observability: { otlp_endpoint: true, ingest_token: true, service_name: null, environment: true },
}

describe('templates', () => {
  it('seeds defaults, skipping secrets and generated values', () => {
    expect(defaultVariables(M)).toEqual({ VERSION: '1.0.0', PORT: '8443', MODEL: 'a', DEBUG: 'false', DATA: '/data' })
    expect(defaultVariables(M, { PORT: '9000' }).PORT).toBe('9000')
  })
  it('a preset takes the template default even when the stack stored an older pin (upgrade)', () => {
    expect(defaultVariables(M, { VERSION: '0.9.0', PORT: '9000' })).toMatchObject({ VERSION: '1.0.0', PORT: '9000' })
  })

  it('validates by type and requires a secret unless the store has it', () => {
    const errors = validateVariables(M, { VERSION: '1', PORT: '99999', MODEL: 'zzz', DEBUG: 'maybe', DATA: 'relative' })
    expect(errors.PORT).toContain('port')
    expect(errors.MODEL).toContain('one of')
    expect(errors.DEBUG).toBeTruthy()
    expect(errors.DATA).toContain('absolute')
    expect(errors.LICENSE).toContain('Required')
    expect(errors.DB_PASSWORD).toBeUndefined()
    expect(validateVariables(M, defaultVariables(M), new Set(['LICENSE']))).toEqual({})
  })

  it('groups General first and Secrets last', () => {
    expect(groupVariables(M).map((g) => g.group)).toEqual(['General', 'Network', 'Secrets'])
  })

  it('renders .env without secrets and with the observability block', () => {
    const env = renderEnv(M, defaultVariables(M), { otlpEndpoint: 'http://p:4318', environment: 'prod' })
    expect(env).toContain('PORT=8443')
    expect(env).toContain('OTEL_EXPORTER_OTLP_ENDPOINT=http://p:4318')
    expect(env).toContain('DEPLOYMENT_ENVIRONMENT=prod')
    expect(env).not.toContain('LICENSE')
    expect(env).not.toContain('DB_PASSWORD')
  })

  it('compares and bumps versions', () => {
    expect(semverCompare('1.10.0', '1.9.2')).toBeGreaterThan(0)
    expect(semverCompare('2.4.1', '2.4.1')).toBe(0)
    expect(nextVersion('2.4.1')).toBe('2.4.2')
    expect(nextVersion('1.2.0-rc1')).toBe('1.2.0-rc2')
  })

  it('renders a manifest as YAML with the photon block', () => {
    const y = manifestToYaml(M)
    expect(y).toContain('id: t')
    expect(y).toContain('  - name: PORT\n    type: port')
    expect(y).toContain('photon:\n  otlp_endpoint: true')
    expect(y).toContain('options: [a, b]')
  })
})

describe('scopeText', () => {
  it('names every host, a site, named hosts, and labels', () => {
    expect(scopeText(null)).toBe('every host')
    expect(scopeText({ kind: 'site', sites: ['jakarta'] })).toBe('site jakarta')
    expect(scopeText({ kind: 'hosts', hosts: ['edge-1', 'edge-2'] })).toBe('hosts edge-1, edge-2')
    expect(scopeText({ kind: 'labels', labels: { env: 'prod' } })).toBe('labels env=prod')
  })
})

describe('requirementChecks', () => {
  const R = { requires: { arch: ['x86_64'], min_docker: '24.0', min_ram_bytes: 4 * 1024 ** 3, min_disk_bytes: 10 * 1024 ** 3 }, ports: [{ name: 'lenz', container: 4030, default_host: 4030, protocol: 'tcp' as const }], volumes: [{ name: 'lenz-db-data', path: '/var/lib/postgresql' }] }
  const facts: DockerFacts = { version: '27.1.1', api_version: '1.46', storage_driver: 'overlay2', os: 'Ubuntu 22.04', arch: 'amd64', containers_running: 3, containers_total: 4, images: 9, disk: { images_bytes: 0, volumes_bytes: 0, build_cache_bytes: 0, reclaimable_bytes: 0, total_bytes: 200 * 1024 ** 3, used_bytes: 82 * 1024 ** 3 } }

  it('marks what the agent reports and leaves the rest to preflight', () => {
    const c = requirementChecks(R, facts)
    expect(c.map((x) => [x.key, x.state])).toEqual([['arch', 'ok'], ['docker', 'ok'], ['ram', 'unknown'], ['disk', 'ok'], ['port:lenz', 'unknown'], ['volume:lenz-db-data', 'unknown']])
    expect(c.find((x) => x.key === 'arch')?.have).toBe('x86_64') // amd64 is x86_64
    expect(c.find((x) => x.key === 'disk')?.have).toBe('118 GB free')
    expect(c.find((x) => x.key === 'ram')?.have).toBe('checked at preflight')
  })

  it('fails an old Docker or a wrong arch, and never fails without facts', () => {
    const c = requirementChecks(R, { ...facts, version: '20.10.7', arch: 'aarch64' })
    expect(c.find((x) => x.key === 'docker')?.state).toBe('fail')
    expect(c.find((x) => x.key === 'arch')?.state).toBe('fail')
    expect(requirementChecks(R, null).every((x) => x.state === 'unknown')).toBe(true)
  })

  it('lists nothing when the template asks for nothing', () => {
    expect(requirementChecks({ requires: {}, ports: [], volumes: [] }, facts)).toEqual([])
  })
})
