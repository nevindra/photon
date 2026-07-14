import { describe, it, expect } from 'vitest'
import { mount, RouterLinkStub } from '@vue/test-utils'
import ErrorIssueList from './ErrorIssueList.vue'
import { EmptyState } from '@/components/ui/empty-state'

const issues = [
  {
    fingerprint: 'abc123',
    exception_type: 'TypeError',
    message: "Cannot read properties of undefined (reading 'price')",
    count: 3420,
    sessions: 1890,
  },
  { fingerprint: 'def456', exception_type: 'ReferenceError', message: 'foo is not defined', count: 12, sessions: 9 },
]

describe('ErrorIssueList', () => {
  it('renders one row per issue with type, message, count, and sessions', () => {
    const w = mount(ErrorIssueList, { props: { issues }, global: { stubs: { RouterLink: RouterLinkStub } } })
    const rows = w.findAll('[data-testid="rum-issue-row"]')
    expect(rows).toHaveLength(2)
    expect(rows[0].text()).toContain('TypeError')
    expect(rows[0].text()).toContain("Cannot read properties of undefined (reading 'price')")
    expect(rows[0].text()).toContain('3,420')
    expect(rows[0].text()).toContain('1,890')
  })

  it('keys rows by fingerprint', () => {
    const w = mount(ErrorIssueList, { props: { issues }, global: { stubs: { RouterLink: RouterLinkStub } } })
    const order = w.findAll('[data-testid="rum-issue-row"]').map((r) => r.attributes('data-fingerprint'))
    expect(order).toEqual(['abc123', 'def456'])
  })

  it('shows an empty state when there are no issues', () => {
    const w = mount(ErrorIssueList, { props: { issues: [] } })
    expect(w.findComponent(EmptyState).exists()).toBe(true)
    expect(w.find('[data-testid="rum-issue-row"]').exists()).toBe(false)
  })

  it('links each row to the issue detail route', () => {
    const wrapper = mount(ErrorIssueList, {
      props: { issues: [{ fingerprint: 'fp1', exception_type: 'TypeError', message: 'x', count: 3, sessions: 2 }], service: 'web' },
      global: { stubs: { RouterLink: RouterLinkStub } },
    })
    const link = wrapper.findAllComponents(RouterLinkStub).find((l) => String(l.props('to')).includes('fp1'))
    expect(link).toBeTruthy()
    expect(link.props('to')).toBe('/rum/web/errors/fp1')
  })
})
