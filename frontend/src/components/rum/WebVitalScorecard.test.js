import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import WebVitalScorecard from './WebVitalScorecard.vue'

describe('WebVitalScorecard', () => {
  it('formats a >=1000ms time metric p75 as seconds and shows the rating pill', () => {
    const w = mount(WebVitalScorecard, {
      props: {
        metric: 'web_vitals.lcp',
        label: 'LCP',
        p75: 2800,
        unit: 'ms',
        rating: 'needs',
        goodMax: 2500,
        poorMin: 4000,
        dist: { good: 58, needs: 31, poor: 11, total: 100 },
      },
    })
    expect(w.text()).toContain('2.8s')
    const pill = w.get('[data-rating="needs"]')
    expect(pill.classes()).toContain('text-sev-warn')
    expect(w.text()).toContain('Good ≤ 2500ms')
  })

  it('formats a sub-1000ms time metric p75 in whole milliseconds', () => {
    const w = mount(WebVitalScorecard, {
      props: { metric: 'web_vitals.inp', label: 'INP', p75: 184, unit: 'ms', rating: 'good' },
    })
    expect(w.text()).toContain('184ms')
    expect(w.get('[data-rating="good"]').classes()).toContain('text-success')
  })

  it('formats CLS as a raw 2dp number, not milliseconds', () => {
    const w = mount(WebVitalScorecard, {
      props: { metric: 'web_vitals.cls', label: 'CLS', p75: 0.0642, unit: '', rating: 'good' },
    })
    expect(w.text()).toContain('0.06')
    expect(w.text()).not.toContain('ms')
  })

  it('renders an em dash and no pill when there is no data', () => {
    const w = mount(WebVitalScorecard, { props: { metric: 'web_vitals.fcp', label: 'FCP', p75: null, rating: null } })
    expect(w.text()).toContain('—')
    expect(w.find('[data-rating]').exists()).toBe(false)
  })
})
