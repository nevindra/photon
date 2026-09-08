import { describe, it, expect } from 'vitest'
import { parsePortainerTemplates } from './portainer'

const V3 = JSON.stringify({
  version: '3',
  templates: [
    { type: 3, title: 'Uptime Kuma', description: 'monitor', categories: ['Monitoring'], repository: { url: 'https://x', stackfile: 'stacks/kuma.yml' }, env: [{ name: 'PORT', label: 'Port', default: '3001' }] },
    { type: 1, title: 'Nginx', image: 'nginx:latest', ports: ['80:80/tcp'], volumes: [{ container: '/usr/share/nginx/html' }], env: [{ name: 'MODE', select: [{ text: 'A', value: 'a', default: true }, { text: 'B', value: 'b' }] }, { name: 'ADMIN_PASSWORD', label: 'Password' }] },
    { type: 2, title: 'Swarm thing' },
    { type: 9, title: 'Weird' },
  ],
})

describe('parsePortainerTemplates', () => {
  it('imports compose stacks and containers, skips Swarm and unknown types', () => {
    const r = parsePortainerTemplates(V3, 'community')
    expect(r.version).toBe(3)
    expect(r.imported.map((t) => t.manifest.id)).toEqual(['uptime-kuma', 'nginx'])
    expect(r.skipped.map((s) => s.title)).toEqual(['Swarm thing', 'Weird'])
    const nginx = r.imported[1]
    expect(nginx.compose_text).toContain('image: nginx:latest')
    expect(nginx.compose_text).toContain('"80:80/tcp"')
    expect(nginx.manifest.variables.find((v) => v.name === 'MODE')?.type).toBe('select')
    expect(nginx.manifest.variables.find((v) => v.name === 'MODE')?.default).toBe('a')
    expect(nginx.manifest.variables.find((v) => v.name === 'ADMIN_PASSWORD')?.type).toBe('secret')
    expect(r.imported[0].manifest.variables[0].type).toBe('port')
    expect(r.imported[0].manifest.categories).toEqual(['monitoring'])
  })

  it('rejects non-JSON and JSON without templates', () => {
    expect(() => parsePortainerTemplates('nope')).toThrow(/JSON/)
    expect(() => parsePortainerTemplates('{"a":1}')).toThrow(/templates/)
  })
})
