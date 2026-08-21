// The SLO card. What is pinned here is judgment, not layout: which tone a budget gets, when a burn
// rate counts as firing, and the two things the card must never hide (a partial window, and a
// latency threshold that was measured somewhere other than where it was set).
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import SloCard from './SloCard.vue'
import type { BurnWindow, Slo, SloBudget } from '@/lib/core/api'

function slo(over: Partial<Slo> = {}): Slo {
  return {
    id: 's',
    service: 'api',
    environment: 'prod',
    operation: '',
    name: 'Availability',
    kind: 'availability',
    target: 0.999,
    threshold_ms: 0,
    window_days: 30,
    ...over,
  }
}

function burn(long: string, short: string, longRate: number, shortRate: number, threshold: number): BurnWindow {
  return {
    long,
    short,
    long_rate: longRate,
    short_rate: shortRate,
    threshold,
    firing: longRate >= threshold && shortRate >= threshold,
  }
}

/** A budget with `remainingFraction` of its allowance left and nothing burning. */
function budget(remainingFraction: number, over: Partial<SloBudget> = {}): SloBudget {
  const total = 1_000_000
  const allowed = 1000
  const remaining = allowed * remainingFraction
  return {
    slo: slo(),
    window_start_ms: 1_700_000_000_000,
    window_end_ms: 1_702_592_000_000,
    partial: false,
    total_events: total,
    bad_events: allowed - remaining,
    sli: (total - (allowed - remaining)) / total,
    budget_events: allowed,
    remaining_events: remaining,
    remaining_fraction: remainingFraction,
    exhausts_in_secs: null,
    fast: burn('1h', '5m', 0, 0, 14.4),
    slow: burn('6h', '30m', 0, 0, 6),
    measured_threshold_ms: null,
    ...over,
  }
}

const mountCard = (b: SloBudget) => mount(SloCard, { props: { budget: b } })

describe('SloCard', () => {
  it('reads healthy when most of the budget is left and nothing is burning', () => {
    const w = mountCard(budget(0.8))
    expect(w.find('[data-testid="slo-card"]').attributes('data-tone')).toBe('ok')
    expect(w.find('[data-testid="slo-remaining"]').text()).toBe('80%')
  })

  it('warns below a quarter of the budget even with nothing burning', () => {
    // A slow leak never trips a burn-rate threshold; it just runs out. The level of the budget is
    // its own signal, separate from the rate.
    expect(mountCard(budget(0.2)).find('[data-testid="slo-card"]').attributes('data-tone')).toBe('warn')
  })

  it('goes critical when the budget is gone, whatever the burn rate says', () => {
    const w = mountCard(budget(-2.4))
    expect(w.find('[data-testid="slo-card"]').attributes('data-tone')).toBe('error')
    // The number is NOT clamped — being 240% over is the fact that matters.
    expect(w.find('[data-testid="slo-remaining"]').text()).toBe('-240%')
    // The bar is, because a bar cannot be negative.
    expect(w.find('[data-testid="slo-bar"]').attributes('style')).toContain('width: 0%')
  })

  it('goes critical on a firing fast burn even with budget left', () => {
    // Half the budget left is comfortable; burning it 20× is not. The rate wins.
    const w = mountCard(budget(0.5, { fast: burn('1h', '5m', 20, 22, 14.4) }))
    expect(w.find('[data-testid="slo-card"]').attributes('data-tone')).toBe('error')
  })

  it('does not fire on a long window the short window has not confirmed', () => {
    // The confirming window is the whole point: a burst that ended forty minutes ago still shows
    // in the 1h rate, and must not be presented as happening now.
    const w = mountCard(budget(0.5, { fast: burn('1h', '5m', 20, 0, 14.4) }))
    expect(w.find('[data-testid="slo-card"]').attributes('data-tone')).toBe('ok')
  })

  it('shows both burn windows, labelled by their long window', () => {
    const w = mountCard(budget(0.5, { fast: burn('1h', '5m', 3.5, 4, 14.4) }))
    expect(w.find('[data-burn="1h"]').text()).toContain('3.5×')
    expect(w.find('[data-burn="6h"]').exists()).toBe(true)
  })

  it('says when the budget runs out, in a unit somebody can act on', () => {
    const w = mountCard(budget(0.4, { exhausts_in_secs: 3 * 3600 + 600 }))
    expect(w.find('[data-testid="slo-exhausts"]').text()).toContain('gone in 3.2h')
  })

  it('says the budget is spent rather than "gone in 0m"', () => {
    const w = mountCard(budget(-0.5, { exhausts_in_secs: 0 }))
    expect(w.find('[data-testid="slo-exhausts"]').text()).toContain('budget spent')
  })

  it('states a partial window instead of passing it off as the full one', () => {
    const w = mountCard(budget(0.9, { partial: true }))
    expect(w.find('[data-testid="slo-partial"]').text()).toContain('part of the window')
  })

  it('says where a latency threshold was actually measured when it differs', () => {
    const w = mountCard(
      budget(0.9, {
        slo: slo({ kind: 'latency', threshold_ms: 300, name: 'Latency' }),
        measured_threshold_ms: 200,
      }),
    )
    expect(w.find('[data-testid="slo-measured-note"]').text()).toContain('measured at 200ms')
  })

  it('says nothing about measurement when the threshold is a bucket edge', () => {
    const w = mountCard(
      budget(0.9, {
        slo: slo({ kind: 'latency', threshold_ms: 200 }),
        measured_threshold_ms: 200,
      }),
    )
    expect(w.find('[data-testid="slo-measured-note"]').exists()).toBe(false)
  })

  it('spells the target the way people say it, not as a stored float', () => {
    const w = mountCard(budget(0.9, { slo: slo({ target: 0.999 }) }))
    expect(w.text()).toContain('99.9% of requests succeed')
    const latency = mountCard(
      budget(0.9, { slo: slo({ kind: 'latency', target: 0.99, threshold_ms: 200 }) }),
    )
    expect(latency.text()).toContain('99% under 200ms')
  })

  it('reads unknown, not healthy, when nothing was measured', () => {
    const w = mountCard(budget(0, { remaining_fraction: null, sli: null, budget_events: 0 }))
    expect(w.find('[data-testid="slo-card"]').attributes('data-tone')).toBe('unknown')
    expect(w.find('[data-testid="slo-remaining"]').text()).toBe('—')
  })
})
