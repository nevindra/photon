// The release timeline. The load-bearing behaviour is the before/after diff: it must refuse to
// compare when it does not hold both sides, and it must judge each metric on its own direction.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import DeploymentTimeline from './DeploymentTimeline.vue'
import type { Deployment, ServiceTimeseriesBucket } from '@/lib/core/api'

const T0 = 1_700_000_000_000

function deploy(over: Partial<Deployment> = {}): Deployment {
  return {
    id: 'api-1.0.0',
    service: 'api',
    environment: 'prod',
    tenant: '',
    version: '1.0.0',
    detected_at_ms: T0,
    source: 'detected',
    commit_sha: null,
    author: null,
    message: null,
    url: null,
    ...over,
  }
}

/**
 * A run of one-minute buckets. `mutate` gets the bucket index and may change the row, which is how
 * a test says "after minute 60, the error rate tripled".
 */
function buckets(
  n: number,
  from: number,
  mutate: (i: number, b: ServiceTimeseriesBucket) => void = () => {},
): ServiceTimeseriesBucket[] {
  return Array.from({ length: n }, (_, i) => {
    const b: ServiceTimeseriesBucket = {
      ts: from + i * 60_000,
      rate: 10,
      error_rate: 0.01,
      count: 600,
      span_count: 900,
      error_count: 6,
      http_4xx: 0,
      http_5xx: 6,
      p50: '1000000',
      p90: '4000000',
      p99: '10000000',
      apdex: 0.95,
      satisfied: 570,
      tolerating: 24,
      frustrated: 6,
    }
    mutate(i, b)
    return b
  })
}

function mountTimeline(props: Record<string, unknown>) {
  return mount(DeploymentTimeline, { props: { deployments: [], ...props } as never })
}

describe('DeploymentTimeline', () => {
  it('says nothing shipped rather than drawing an empty rail', () => {
    const w = mountTimeline({ deployments: [] })
    expect(w.find('[data-testid="deployment-empty"]').exists()).toBe(true)
    expect(w.find('[data-testid="deployment-row"]').exists()).toBe(false)
  })

  it('orders releases newest first regardless of the order handed in', () => {
    const w = mountTimeline({
      deployments: [
        deploy({ id: 'old', version: '1.0.0', detected_at_ms: T0 }),
        deploy({ id: 'new', version: '1.1.0', detected_at_ms: T0 + 3_600_000 }),
      ],
    })
    const ids = w.findAll('[data-testid="deployment-row"]').map((r) => r.attributes('data-deploy'))
    expect(ids).toEqual(['new', 'old'])
  })

  it('refuses a comparison when it does not hold a full window on both sides', () => {
    // Buckets start AT the release, so the "before" side does not exist. Diffing half a window
    // against a full one would invent a regression.
    const w = mountTimeline({
      deployments: [deploy()],
      buckets: buckets(60, T0),
      compareMinutes: 30,
    })
    expect(w.find('[data-testid="deployment-deltas"]').exists()).toBe(false)
    expect(w.find('[data-testid="deployment-no-compare"]').text()).toContain('window before it')
  })

  it('compares when both sides are covered', () => {
    const w = mountTimeline({
      deployments: [deploy()],
      buckets: buckets(120, T0 - 60 * 60_000),
      compareMinutes: 30,
    })
    expect(w.find('[data-testid="deployment-deltas"]').exists()).toBe(true)
    expect(w.findAll('[data-delta]').map((d) => d.attributes('data-delta'))).toEqual([
      'error_rate',
      'p99',
      'apdex',
      'rate',
    ])
  })

  it('paints a rising error rate as worse and a rising Apdex as better', () => {
    const rows = buckets(120, T0 - 60 * 60_000, (i, b) => {
      if (b.ts >= T0) {
        b.error_count = 60 // 1% → 10%
        b.error_rate = 0.1
        b.apdex = 0.99 // …while Apdex improved, which must be tinted the other way
      } else {
        b.apdex = 0.8
      }
    })
    const w = mountTimeline({ deployments: [deploy()], buckets: rows, compareMinutes: 30 })
    expect(w.find('[data-delta="error_rate"] span:last-child').classes()).toContain('text-sev-error')
    expect(w.find('[data-delta="apdex"] span:last-child').classes()).toContain('text-success')
  })

  it('leaves throughput untinted — more traffic is not an improvement', () => {
    const rows = buckets(120, T0 - 60 * 60_000, (i, b) => {
      if (b.ts >= T0) b.rate = 100 // 10x the load
    })
    const w = mountTimeline({ deployments: [deploy()], buckets: rows, compareMinutes: 30 })
    expect(w.find('[data-delta="rate"] span:last-child').classes()).toContain('text-muted-foreground')
  })

  it('treats a move under the noise floor as unchanged', () => {
    const rows = buckets(120, T0 - 60 * 60_000, (i, b) => {
      if (b.ts >= T0) b.error_count = 6.1 // +1.7%, well under the 5% noise floor
    })
    const w = mountTimeline({ deployments: [deploy()], buckets: rows, compareMinutes: 30 })
    expect(w.find('[data-delta="error_rate"] span:last-child').classes()).toContain(
      'text-muted-foreground',
    )
  })

  it('labels an auto-detected release as inferred and a CI one as reported', () => {
    const w = mountTimeline({
      deployments: [
        deploy({ id: 'a', detected_at_ms: T0 + 1000 }),
        deploy({ id: 'b', source: 'ci', commit_sha: 'abc1234def', author: 'nevindra' }),
      ],
    })
    expect(w.find('[data-deploy="a"] [data-source="detected"]').text()).toContain('auto-detected')
    expect(w.find('[data-deploy="b"] [data-source="ci"]').text()).toContain('from CI')
    // Commit shown short, never the full 40 characters.
    expect(w.find('[data-deploy="b"]').text()).toContain('abc1234')
    expect(w.find('[data-deploy="b"]').text()).not.toContain('abc1234def')
  })

  it('emits the release on a version click so the caller can drill in', async () => {
    const w = mountTimeline({ deployments: [deploy({ version: '2.0.0' })] })
    await w.find('[data-testid="deployment-row"] button').trigger('click')
    expect((w.emitted('select')?.[0][0] as Deployment).version).toBe('2.0.0')
  })

  it('calls a release younger than the comparison window too recent', () => {
    const w = mountTimeline({
      deployments: [deploy({ detected_at_ms: T0 + 110 * 60_000 })],
      buckets: buckets(120, T0),
      compareMinutes: 30,
    })
    expect(w.find('[data-testid="deployment-no-compare"]').text()).toContain('too recent')
  })

  it('lists releases without any comparison when no RED data was handed in', () => {
    const w = mountTimeline({ deployments: [deploy()], buckets: [] })
    expect(w.find('[data-testid="deployment-row"]').exists()).toBe(true)
    expect(w.find('[data-testid="deployment-no-compare"]').text()).toContain('no RED data')
  })
})
