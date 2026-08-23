// The per-service footprint table. What is pinned: the ordering the page exists for, and the two
// caveats it must never soften — that the bytes are divided rather than measured, and that the
// metrics bytes belong to somebody even though no service is named.
import { describe, it, expect, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import DataServices from './DataServices.vue'
import type { ServiceUsageResult, ServiceUsageRow } from '@/lib/core/api'

const { usage } = vi.hoisted(() => ({ usage: { value: null as ServiceUsageResult | null } }))
vi.mock('@/lib/data/dataQueries', () => ({
  useServiceUsage: () => ({
    data: { value: usage.value },
    isFetching: { value: false },
  }),
}))

function row(service: string, logRows: number, spanRows: number): ServiceUsageRow {
  const logs = { rows: logRows, bytes: logRows * 40 }
  const traces = { rows: spanRows, bytes: spanRows * 400 }
  return {
    service,
    signals: { logs, traces },
    rows_total: logRows + spanRows,
    bytes_total: logs.bytes + traces.bytes,
  }
}

const basis = (bytesPerRow: number, storeBytes: number, attributed: boolean) => ({
  store_rows: Math.round(storeBytes / bytesPerRow),
  store_bytes: storeBytes,
  bytes_per_row: bytesPerRow,
  attributed,
  capped: false,
})

function result(rows: ServiceUsageRow[], metricsBytes = 0): ServiceUsageResult {
  const attributed = rows.reduce((n, r) => n + r.bytes_total, 0)
  return {
    window: { start: '0', end: '1' },
    rows,
    estimated: true,
    signals: {
      logs: basis(40, attributed, true),
      traces: basis(400, 0, true),
      metrics: basis(96, metricsBytes, false),
    },
    notes: { metrics: 'Metrics are not attributed to services: a metric series is keyed by name.' },
  }
}

async function mountTable(r: ServiceUsageResult | null) {
  usage.value = r
  const w = mount(DataServices)
  await flushPromises()
  return w
}

describe('DataServices', () => {
  it('renders one row per service, in the order the API sent', async () => {
    // The API already sorts by estimated footprint. Re-sorting here would be a second opinion, and
    // the two could drift.
    const w = await mountTable(result([row('api', 1_000, 9_000), row('web', 40_000, 100)]))
    const order = w
      .findAll('[data-testid="usage-service-row"]')
      .map((r) => r.attributes('data-service'))
    expect(order).toEqual(['api', 'web'])
  })

  it('always says the footprint is an estimate, and shows the divisor it used', async () => {
    // A divided number presented as a measured one is how a capacity plan goes wrong quietly.
    const w = await mountTable(result([row('api', 1_000, 9_000)]))
    const note = w.find('[data-testid="usage-estimate-note"]').text()
    expect(note).toContain('estimate')
    expect(note).toContain('40.0 B/log row')
    expect(note).toContain('400.0 B/span row')
  })

  it('carries the metrics explanation through from the API rather than paraphrasing it', async () => {
    const w = await mountTable(result([row('api', 1, 1)]))
    expect(w.find('[data-testid="usage-metrics-note"]').text()).toContain('keyed by name')
  })

  it('accounts for the bytes it did not attribute instead of leaving them unexplained', async () => {
    // Someone reading a cost page must not conclude the missing metrics bytes are waste.
    const w = await mountTable(result([row('api', 10, 10)], 5_000_000))
    expect(w.find('[data-testid="usage-unattributed"]').exists()).toBe(true)
    expect(w.find('[data-testid="usage-unattributed"]').text()).toContain('not attributed')
  })

  it('shows no unattributed line when everything is accounted for', async () => {
    const w = await mountTable(result([row('api', 10, 10)], 0))
    expect(w.find('[data-testid="usage-unattributed"]').exists()).toBe(false)
  })

  it('says the window is empty rather than drawing an empty table', async () => {
    const w = await mountTable(result([]))
    expect(w.find('[data-testid="usage-service-row"]').exists()).toBe(false)
    expect(w.text()).toContain('Nothing written by a named service')
  })

  it('survives a response that has not arrived yet', async () => {
    const w = await mountTable(null)
    expect(w.find('[data-testid="usage-service-row"]').exists()).toBe(false)
    expect(w.find('[data-testid="usage-estimate-note"]').exists()).toBe(true)
  })
})
