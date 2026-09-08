// Compose-file helpers that need no server: the dangerous-construct lint (R8.4), a service-name
// scan, `.env` parsing, and the starter file for a new stack. Pure, line-based — not a YAML
// parser. The server renders and validates for real; this is what lets the editor warn while
// the operator is still typing, and what the mock's `validate` answers with.
import type { ComposeWarning } from '@/lib/core/api'

export const DEFAULT_COMPOSE = `services:
  app:
    image: ghcr.io/nodeflux/example:1.0.0
    restart: unless-stopped
    ports:
      - "8080:8080"
    environment:
      OTEL_EXPORTER_OTLP_ENDPOINT: \${OTEL_EXPORTER_OTLP_ENDPOINT}
      OTEL_SERVICE_NAME: example
    volumes:
      - app_data:/var/lib/app

volumes:
  app_data:
`

export const DEFAULT_ENV = `# Variables referenced by compose.yaml. Secrets belong in Photon's secret store, not here.
OTEL_EXPORTER_OTLP_ENDPOINT=http://photon:4318
`

// Bind mounts are allowed under these prefixes; anything else on the host filesystem is flagged.
export const ALLOWED_BIND_PREFIXES = ['/opt/photon', '/var/lib/photon', '/srv', '/data']

interface Line {
  n: number
  indent: number
  text: string
}

function lines(text: string): Line[] {
  return text.split(/\r?\n/).map((raw, i) => ({ n: i + 1, indent: raw.length - raw.trimStart().length, text: raw.trim() }))
}

/** Service names under the top-level `services:` block. */
export function composeServices(text: string): string[] {
  const out: string[] = []
  let inServices = false
  let serviceIndent = -1
  for (const l of lines(text)) {
    if (!l.text || l.text.startsWith('#')) continue
    if (l.indent === 0) {
      inServices = l.text === 'services:'
      serviceIndent = -1
      continue
    }
    if (!inServices) continue
    if (serviceIndent === -1) serviceIndent = l.indent
    if (l.indent === serviceIndent && /^[A-Za-z0-9_.-]+:\s*$/.test(l.text)) out.push(l.text.slice(0, -1))
  }
  return out
}

/** The service a given line belongs to, walking back to the nearest service header. */
function serviceAt(all: Line[], idx: number): string | null {
  let inServices = false
  let serviceIndent = -1
  let current: string | null = null
  for (let i = 0; i <= idx; i++) {
    const l = all[i]
    if (!l.text || l.text.startsWith('#')) continue
    if (l.indent === 0) {
      inServices = l.text === 'services:'
      serviceIndent = -1
      current = null
      continue
    }
    if (!inServices) continue
    if (serviceIndent === -1) serviceIndent = l.indent
    if (l.indent === serviceIndent && /^[A-Za-z0-9_.-]+:\s*$/.test(l.text)) current = l.text.slice(0, -1)
  }
  return current
}

/**
 * The constructs an operator must acknowledge before a compose file ships to a customer host.
 * Each warning's `id` is stable across edits of the same construct so an acknowledgement sticks.
 */
export function lintCompose(text: string): ComposeWarning[] {
  const all = lines(text)
  const out: ComposeWarning[] = []
  const push = (kind: ComposeWarning['kind'], severity: ComposeWarning['severity'], i: number, detail: string) => {
    const service = serviceAt(all, i)
    out.push({ id: `${kind}:${service ?? '-'}`, kind, severity, service, line: all[i].n, detail })
  }
  let inVolumesList = false
  for (let i = 0; i < all.length; i++) {
    const l = all[i]
    if (!l.text || l.text.startsWith('#')) continue
    if (/^privileged:\s*true\b/.test(l.text)) push('privileged', 'error', i, 'Runs as root on the host with every capability. Almost nothing needs this.')
    if (/^network_mode:\s*["']?host["']?\s*$/.test(l.text)) push('host_network', 'warning', i, "Shares the host's network namespace: every port the container opens is open on the host.")
    if (/^pid:\s*["']?host["']?\s*$/.test(l.text)) push('host_pid', 'warning', i, "Sees and can signal every process on the host.")
    if (/^cap_add:/.test(l.text)) push('cap_add', 'warning', i, 'Adds Linux capabilities beyond the default set.')
    if (/docker\.sock/.test(l.text)) push('docker_sock', 'error', i, 'Mounts the Docker socket: full control of the daemon, which is root on the host.')
    if (/^volumes:\s*$/.test(l.text)) inVolumesList = true
    else if (l.indent === 0 || (!l.text.startsWith('-') && inVolumesList && !/^volumes:/.test(l.text))) inVolumesList = false
    if (inVolumesList && l.text.startsWith('-')) {
      const spec = l.text.slice(1).trim().replace(/^["']|["']$/g, '')
      const src = spec.split(':')[0]
      if (src.startsWith('/') && !/docker\.sock/.test(spec) && !ALLOWED_BIND_PREFIXES.some((p) => src === p || src.startsWith(p + '/'))) {
        push('bind_mount_outside', 'warning', i, `Bind-mounts ${src} from the host, outside the allowed prefixes (${ALLOWED_BIND_PREFIXES.join(', ')}).`)
      }
    }
  }
  // De-duplicate by id (a service with two docker.sock mounts is one acknowledgement).
  const seen = new Set<string>()
  return out.filter((w) => (seen.has(w.id) ? false : (seen.add(w.id), true)))
}

/** `.env` → { KEY: value }, ignoring comments and blanks; quotes stripped. */
export function parseEnv(text: string): Record<string, string> {
  const out: Record<string, string> = {}
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const eq = line.indexOf('=')
    if (eq <= 0) continue
    const key = line.slice(0, eq).trim()
    let val = line.slice(eq + 1).trim()
    if ((val.startsWith('"') && val.endsWith('"')) || (val.startsWith("'") && val.endsWith("'"))) val = val.slice(1, -1)
    out[key] = val
  }
  return out
}

/** `${VAR}` / `${VAR:-default}` / `$VAR` references in a compose file, deduplicated. */
export function composeVariableRefs(text: string): string[] {
  const refs = new Set<string>()
  // `$$` is Compose's escape for a literal `$` — never a reference. Modifiers: `:-` `-` `:?` `?`
  // `:+` `+` (defaults, errors, alternates), all with an arbitrary word after them.
  const unescaped = text.replace(/\$\$/g, '')
  for (const m of unescaped.matchAll(/\$\{([A-Za-z_][A-Za-z0-9_]*)(?::?[-?+][^}]*)?\}|\$([A-Za-z_][A-Za-z0-9_]*)/g)) refs.add(m[1] ?? m[2])
  return Array.from(refs)
}

/** Variables the compose file references that neither `.env` nor the secret store provides. */
export function missingVariables(compose: string, env: Record<string, string>, secrets: string[] = []): string[] {
  const have = new Set([...Object.keys(env), ...secrets])
  return composeVariableRefs(compose).filter((v) => !have.has(v))
}

// Docker's own recognised compose filenames, in lookup order (R4.2).
export const COMPOSE_FILENAMES = ['compose.yaml', 'compose.yml', 'docker-compose.yaml', 'docker-compose.yml']

export function isComposeFile(path: string): boolean {
  return COMPOSE_FILENAMES.includes(path)
}

// Arcane's caps (R4.4): they exist for a reason.
export const FILE_MAX_BYTES = 10 * 1024 * 1024
export const MAX_ENTRIES = 2000
export const MAX_DEPTH = 20

/** A path is safe when it stays inside the stack folder: relative, no `..`, no absolute root. */
export function isSafePath(path: string): boolean {
  if (!path || path.startsWith('/') || path.includes('\\')) return false
  const parts = path.split('/')
  if (parts.some((p) => p === '..' || p === '' || p === '.')) return false
  return parts.length <= MAX_DEPTH
}
