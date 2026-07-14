import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { ref } from 'vue'
import { mockStorage, mockUsageSeries } from '@/lib/core/mock'

// Drive DataOverview off the storage/usage composables synchronously by stubbing dataQueries — the
// same shape TanStack Query returns (a `data` ref + an `isLoading` ref). `mockOverviewState` holds
// the storage ref so each `mountOverview` call can inject a different `{ signals, durable }` corpus
// (both names are whitelisted inside a vi.mock factory: `mock*`-prefixed and imported bindings).
const mockOverviewState = { storage: ref(null) }
vi.mock('@/lib/data/dataQueries', () => ({
  useStorage: () => ({ data: mockOverviewState.storage, isLoading: ref(false) }),
  useUsageSeries: () => ({ data: ref(mockUsageSeries('24h')), isLoading: ref(false) }),
  // Consumed by DataOverview but the stubbed useUsageSeries ignores its arg — any value satisfies the import.
  usageWindow: ref('24h'),
}))

import DataOverview from '@/components/data/DataOverview.vue'

function mountOverview(overrides = {}) {
  const storage = structuredClone(mockStorage)
  Object.assign(storage, overrides) // e.g. override `durable` to the not-configured shape
  mockOverviewState.storage = ref(storage)
  return mount(DataOverview)
}

describe('DataOverview', () => {
  it('sums total hot bytes across signals into the footprint tile', () => {
    const w = mountOverview()
    // 210M + 64M + 12M hot bytes -> a MB/GB value via formatBytes.
    expect(w.get('[data-testid="tile-hot"]').text()).toMatch(/MB|GB/)
  })

  it('hides the durable tile when durable.configured is false', () => {
    const w = mountOverview({ durable: { configured: false, pending: 0, last_replicated_ms: null } })
    expect(w.find('[data-testid="tile-durable"]').exists()).toBe(false)
  })

  it('shows the durable tile when durable.configured is true', () => {
    const w = mountOverview()
    expect(w.find('[data-testid="tile-durable"]').exists()).toBe(true)
  })

  it('renders a storage composition legend entry per signal with non-zero bytes', () => {
    const w = mountOverview()
    // mockStorage has logs/traces/metrics all with bytes > 0 -> three legend rows. The legend text
    // is lowercase (`capitalize` is a CSS transform, not a DOM transform).
    const composition = w.get('[data-testid="storage-composition"]').text()
    expect(composition).toContain('logs')
    expect(composition).toContain('traces')
    expect(composition).toContain('metrics')
    expect(w.text()).toMatch(/On-disk share by signal/)
  })

  it('omits a signal from the composition legend when its bytes are 0', () => {
    const w = mountOverview({
      signals: {
        logs: { file_count: 12, total_rows: 4_200_000, bytes: 210_000_000, durable_bytes: 176_000_000 },
        traces: { file_count: 5, total_rows: 900_000, bytes: 64_000_000, durable_bytes: 38_000_000 },
        metrics: { file_count: 0, total_rows: 0, bytes: 0, durable_bytes: 0 },
      },
    })
    // The composition legend drops zero-byte signals — scope to its testid so we don't collide with
    // the (unrelated, statically-mocked) usage-chart legends elsewhere on the page.
    expect(w.get('[data-testid="storage-composition"]').text()).not.toContain('metrics')
  })

  it('shows a neutral empty state when there is no on-disk data at all', () => {
    const w = mountOverview({
      signals: {
        logs: { file_count: 0, total_rows: 0, bytes: 0, durable_bytes: 0 },
        traces: { file_count: 0, total_rows: 0, bytes: 0, durable_bytes: 0 },
        metrics: { file_count: 0, total_rows: 0, bytes: 0, durable_bytes: 0 },
      },
    })
    expect(w.get('[data-testid="storage-composition"]').text()).toContain('No data yet.')
  })
})
