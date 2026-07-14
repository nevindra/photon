import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import RumIssuesFeed from './RumIssuesFeed.vue'
import type { Issue } from '@/lib/rum/rumSummary'

const issues: Issue[] = [
  { app: 'web-storefront', fingerprint: 'f1', exception_type: 'TypeError', message: "Cannot read 'price'", count: 3420, sessions: 1890 },
]

describe('RumIssuesFeed', () => {
  it('shows the exception type, message, app tag and counts', () => {
    const w = mount(RumIssuesFeed, { props: { issues } })
    const row = w.get('[data-testid="rum-issue"]')
    expect(row.text()).toContain('TypeError')
    expect(row.text()).toContain("Cannot read 'price'")
    expect(row.text()).toContain('web-storefront')
    expect(row.text()).toContain('3,420')
    expect(row.text()).toContain('1,890 sessions')
  })

  it('emits open with the app on click', async () => {
    const w = mount(RumIssuesFeed, { props: { issues } })
    await w.get('[data-testid="rum-issue"]').trigger('click')
    expect(w.emitted('open')?.[0]).toEqual(['web-storefront'])
  })
})
