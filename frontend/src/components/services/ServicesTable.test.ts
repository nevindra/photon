// The index table. Two things here are load-bearing rather than cosmetic:
//
//   - A stale Site's child row must render `—` and Unknown. `0/s`, `0.00%` and a green OK is a
//     claim about a Site we have not heard from (ADR-0004), and it is the exact failure the whole
//     fleet-board redesign exists to prevent.
//   - The row EXPANDS on click and only the service NAME navigates. Making the whole row navigate
//     is how the old table made comparison impossible.
import { describe, it, expect, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import ServicesTable from './ServicesTable.vue'
import { mergeFleet } from '@/lib/services/servicesOverview'
import type { OverviewRow, OverviewSite, ServicesOverview } from '@/lib/core/api'

vi.mock('@/lib/core/api', () => ({
  api: { mock: false, red: vi.fn().mockResolvedValue([]), serviceDependencies: vi.fn().mockResolvedValue({ database: [], external: [] }) },
}))

function orow(over: Partial<OverviewRow> = {}): OverviewRow {
  return {
    service: 'api', tenant: null, environment: null, operation: null,
    count: 1000, rate: 10, span_count: 1400, basis: 'entry',
    error_count: 0, error_rate: 0, http_4xx: 0, http_5xx: 0,
    satisfied: 1000, tolerating: 0, frustrated: 0, apdex: 1,
    p50: '1000000', p90: '2000000', p99: '3000000',
    spark: [], error_spark: [], ...over,
  }
}
const osite = (over: Partial<OverviewSite> = {}): OverviewSite => ({
  name: 'jkt', status: 'up', last_seen_ms: 1, mode: 'summary', ui_url: null, ...over,
})
const overview = (rows: OverviewRow[], sites: OverviewSite[] = []): ServicesOverview => ({
  source: 'raw', window: { start: '0', end: '1', buckets: 30 }, sites, rows,
})

// The table renders Reka Tooltips, which need a real TooltipProvider ancestor — mounting the
// component bare throws on injection.
function mountTable(rows: ReturnType<typeof mergeFleet>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  return mount(
    {
      components: { TooltipProvider, ServicesTable },
      props: ['rows'],
      template:
        '<TooltipProvider><ServicesTable :rows="rows" :window-secs="100" start-ns="0" end-ns="1" /></TooltipProvider>',
    },
    { props: { rows }, global: { plugins: [[VueQueryPlugin, { queryClient }]] } },
  )
}

describe('ServicesTable', () => {
  it('sorts worst first by level, then by error rate', () => {
    const rows = mergeFleet(
      overview([
        orow({ service: 'ok-svc' }),
        orow({ service: 'crit-a', error_rate: 0.5, error_count: 500 }),
        orow({ service: 'crit-b', error_rate: 0.9, error_count: 900 }),
        orow({ service: 'warn-svc', error_rate: 0.02, error_count: 20 }),
      ]),
    )
    const w = mountTable(rows)
    expect(w.findAll('[data-testid="service-row"]').map((r) => r.attributes('data-service'))).toEqual([
      'crit-b', 'crit-a', 'warn-svc', 'ok-svc',
    ])
  })

  it('badges span-based only when the count did not come from entry spans', () => {
    const w = mountTable(mergeFleet(overview([orow({ service: 'a' }), orow({ service: 'b', basis: 'all_spans' })])))
    const rows = w.findAll('[data-testid="service-row"]')
    const badged = rows.filter((r) => r.find('[data-testid="span-based-badge"]').exists())
    expect(badged).toHaveLength(1)
    expect(badged[0].attributes('data-service')).toBe('b')
  })

  it('shows Spread only when more than one Site reports', () => {
    const one = mountTable(mergeFleet(overview([orow({ tenant: 'jkt', error_rate: 0.5, error_count: 500 })], [osite()])))
    expect(one.find('[data-testid="spread-badge"]').exists()).toBe(false)

    const many = mountTable(
      mergeFleet(
        overview(
          [orow({ tenant: 'jkt', error_rate: 0.5, error_count: 500 }), orow({ tenant: 'sgp' })],
          [osite({ name: 'jkt' }), osite({ name: 'sgp' })],
        ),
      ),
    )
    expect(many.find('[data-testid="spread-badge"]').text()).toContain('1 of 2')
  })

  it('expands on row click and collapses on a second click', async () => {
    const rows = mergeFleet(overview([orow({ tenant: 'jkt' }), orow({ tenant: 'sgp' })], [osite({ name: 'jkt' }), osite({ name: 'sgp' })]))
    const w = mountTable(rows)
    expect(w.find('[data-testid="row-expansion"]').exists()).toBe(false)
    await w.find('[data-testid="service-row"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="row-expansion"]').exists()).toBe(true)
    await w.find('[data-testid="service-row"]').trigger('click')
    expect(w.find('[data-testid="row-expansion"]').exists()).toBe(false)
  })

  it('keeps only one row expanded at a time', async () => {
    const w = mountTable(mergeFleet(overview([orow({ service: 'a' }), orow({ service: 'b' })])))
    const rows = w.findAll('[data-testid="service-row"]')
    await rows[0].trigger('click')
    await rows[1].trigger('click')
    await flushPromises()
    expect(w.findAll('[data-testid="row-expansion"]')).toHaveLength(1)
  })

  it('renders a stale Site’s child row as unknown dashes, never zero and OK', async () => {
    const rows = mergeFleet(
      overview(
        [orow({ tenant: 'jkt' }), orow({ tenant: 'sgp', count: 0, error_count: 0, apdex: null, p99: '0' })],
        [osite({ name: 'jkt' }), osite({ name: 'sgp', status: 'stale' })],
      ),
    )
    const w = mountTable(rows)
    await w.find('[data-testid="service-row"]').trigger('click')
    await flushPromises()
    const sgp = w.find('[data-testid="site-child-row"][data-site="sgp"]')
    expect(sgp.attributes('data-level')).toBe('unknown')
    expect(sgp.text()).toContain('—')
    expect(sgp.text()).not.toContain('0.00%')
  })

  it('navigates from the service NAME, not from the row', async () => {
    const w = mountTable(mergeFleet(overview([orow({ service: 'api' })])))
    await w.find('[data-testid="service-row"]').trigger('click')
    expect(w.findComponent(ServicesTable).emitted('open-service')).toBeUndefined()
    await w.find('[data-testid="service-link"]').trigger('click')
    expect(w.findComponent(ServicesTable).emitted('open-service')?.[0]?.[0]).toMatchObject({ service: 'api' })
  })

  it('splits the error cell by HTTP class so a caller’s bad request is not our fault', () => {
    const w = mountTable(mergeFleet(overview([orow({ error_count: 30, error_rate: 0.03, http_4xx: 100, http_5xx: 30 })])))
    const cell = w.find('[data-testid="service-row"]').text()
    expect(cell).toContain('30 5xx')
    expect(cell).toContain('100 4xx')
  })

  it('reads Traffic against the window, and shows total spans beside it', () => {
    // 1000 requests over 100s = 10/s, with 1.4k spans recorded (ADR-0003's second number).
    const w = mountTable(mergeFleet(overview([orow()])))
    const text = w.find('[data-testid="service-row"]').text()
    expect(text).toContain('10.0/s')
    expect(text).toContain('1.4k spans')
  })

  it('shows an empty state and no rows when nothing is in range', () => {
    const w = mountTable([])
    expect(w.findAll('[data-testid="service-row"]')).toHaveLength(0)
    expect(w.text()).toContain('No services in range')
  })
})
