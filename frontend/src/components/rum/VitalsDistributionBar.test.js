import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import VitalsDistributionBar from './VitalsDistributionBar.vue'

describe('VitalsDistributionBar', () => {
  it('fills each segment to its proportional share', () => {
    const w = mount(VitalsDistributionBar, { props: { good: 50, needs: 30, poor: 20 } })
    expect(w.get('[data-testid="dist-good"]').attributes('style')).toContain('width: 50%')
    expect(w.get('[data-testid="dist-needs"]').attributes('style')).toContain('width: 30%')
    expect(w.get('[data-testid="dist-poor"]').attributes('style')).toContain('width: 20%')
  })

  it('applies the literal tone classes to each segment', () => {
    const w = mount(VitalsDistributionBar, { props: { good: 1, needs: 1, poor: 1 } })
    expect(w.get('[data-testid="dist-good"]').classes()).toContain('bg-success')
    expect(w.get('[data-testid="dist-needs"]').classes()).toContain('bg-sev-warn')
    expect(w.get('[data-testid="dist-poor"]').classes()).toContain('bg-sev-error')
  })

  it('treats an all-zero distribution as 0% everywhere (no NaN widths)', () => {
    const w = mount(VitalsDistributionBar, { props: { good: 0, needs: 0, poor: 0 } })
    expect(w.get('[data-testid="dist-good"]').attributes('style')).toContain('width: 0%')
    expect(w.get('[data-testid="dist-poor"]').attributes('style')).toContain('width: 0%')
  })
})
