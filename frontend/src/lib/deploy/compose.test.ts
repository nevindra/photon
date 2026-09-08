import { describe, it, expect } from 'vitest'
import { lintCompose, composeServices, parseEnv, composeVariableRefs, missingVariables, isSafePath, isComposeFile } from './compose'

const COMPOSE = `services:
  api:
    image: x:1
    privileged: true
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - /etc/ssl:/certs:ro
      - /opt/photon/data:/data
      - app:/var/lib/app
  worker:
    image: y:\${VERSION}
    network_mode: host
    environment:
      TOKEN: \${secret:TOKEN}
      LEVEL: \${LOG_LEVEL:-info}
volumes:
  app:
`

describe('compose helpers', () => {
  it('lists services under services: only', () => {
    expect(composeServices(COMPOSE)).toEqual(['api', 'worker'])
    expect(composeServices('volumes:\n  a:\n')).toEqual([])
  })

  it('flags the dangerous constructs per service, once each, with stable ids', () => {
    const w = lintCompose(COMPOSE)
    const ids = w.map((x) => x.id)
    expect(ids).toContain('privileged:api')
    expect(ids).toContain('docker_sock:api')
    expect(ids).toContain('bind_mount_outside:api')
    expect(ids).toContain('host_network:worker')
    expect(ids.filter((i) => i === 'bind_mount_outside:api')).toHaveLength(1)
    expect(w.find((x) => x.id === 'privileged:api')?.severity).toBe('error')
    expect(w.find((x) => x.id === 'host_network:worker')?.severity).toBe('warning')
    expect(w.find((x) => x.id === 'bind_mount_outside:api')?.detail).toContain('/etc/ssl')
    // /opt/photon and named volumes are fine
    expect(lintCompose('services:\n  a:\n    image: x\n    volumes:\n      - /opt/photon/x:/x\n      - data:/d\n')).toEqual([])
  })

  it('parses .env and finds unresolved references', () => {
    expect(parseEnv('# c\nA=1\nB="two words"\nbad\nC=\'q\'\n')).toEqual({ A: '1', B: 'two words', C: 'q' })
    // `${secret:NAME}` is a secret-store reference, not an env variable, and is not a ref.
    expect(composeVariableRefs(COMPOSE)).toEqual(['VERSION', 'LOG_LEVEL'])
    expect(missingVariables(COMPOSE, { VERSION: '1' })).toEqual(['LOG_LEVEL'])
    expect(missingVariables(COMPOSE, { VERSION: '1', LOG_LEVEL: 'debug' })).toEqual([])
  })

  it('keeps paths inside the stack folder', () => {
    expect(isSafePath('compose.yaml')).toBe(true)
    expect(isSafePath('conf/nginx.conf')).toBe(true)
    expect(isSafePath('../etc/passwd')).toBe(false)
    expect(isSafePath('/etc/passwd')).toBe(false)
    expect(isSafePath('a//b')).toBe(false)
    expect(isComposeFile('docker-compose.yml')).toBe(true)
    expect(isComposeFile('stack.yaml')).toBe(false)
  })
})
