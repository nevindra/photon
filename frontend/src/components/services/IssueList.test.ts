// The Issues table. What is worth pinning is the identity rule: a row's detail link must carry
// the row's OWN service, because on the fleet list two rows can share a fingerprint and differ
// only by service — linking through the page's service would open the wrong Issue.
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import IssueList from './IssueList.vue'
import type { BackendIssue } from '@/lib/core/api'

const routes = [
  { path: '/services', component: { template: '<div />' } },
  { path: '/services/:service', component: { template: '<div />' } },
  { path: '/services/:service/errors', component: { template: '<div />' } },
  { path: '/services/:service/errors/:fingerprint', component: { template: '<div />' } },
]

function issue(over: Partial<BackendIssue> = {}): BackendIssue {
  return {
    service: 'api',
    fingerprint: 'abc123',
    error_type: 'NullPointerException',
    message: 'Cannot read property id of undefined',
    count: 137,
    traces: 120,
    operations: 1,
    first_seen: '1000000000000000000',
    last_seen: '1000000060000000000',
    trace_id: 'a'.repeat(32),
    ...over,
  }
}

async function mountList(issues: BackendIssue[], showService = false) {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/services/errors?range=6h')
  await router.isReady()
  return mount(IssueList, { props: { issues, showService }, global: { plugins: [router] } })
}

describe('IssueList', () => {
  it('renders one row per issue', async () => {
    const w = await mountList([issue(), issue({ fingerprint: 'def456' })])
    expect(w.findAll('[data-testid="backend-issue-row"]')).toHaveLength(2)
  })

  it('links each row to its OWN service, not the page it is rendered on', async () => {
    const w = await mountList(
      [issue({ service: 'api', fingerprint: 'shared' }), issue({ service: 'worker', fingerprint: 'shared' })],
      true,
    )
    const rows = w.findAll('[data-testid="backend-issue-row"]')
    expect(rows[0].find('a[href*="/errors/shared"]').attributes('href')).toContain('/services/api/errors/shared')
    expect(rows[1].find('a[href*="/errors/shared"]').attributes('href')).toContain('/services/worker/errors/shared')
  })

  it('shows the service column only when asked', async () => {
    const withCol = await mountList([issue()], true)
    const withoutCol = await mountList([issue()], false)
    expect(withCol.text()).toContain('Service')
    expect(withoutCol.text()).not.toContain('Service')
  })

  it('flags an issue that reaches more than one operation', async () => {
    const one = await mountList([issue({ operations: 1 })])
    const many = await mountList([issue({ operations: 3 })])
    expect(one.text()).not.toContain('across')
    expect(many.text()).toContain('across 3 operations')
  })

  it('carries the active window onto every link', async () => {
    const w = await mountList([issue()], true)
    const hrefs = w.findAll('a').map((a) => a.attributes('href') ?? '')
    expect(hrefs.filter((h) => h.startsWith('/services')).every((h) => h.includes('range=6h'))).toBe(true)
  })

  it('says nothing broke rather than rendering an empty table', async () => {
    const w = await mountList([])
    expect(w.text()).toContain('No issues in range')
    expect(w.findAll('[data-testid="backend-issue-row"]')).toHaveLength(0)
  })

  it('survives a last_seen it cannot parse instead of throwing', async () => {
    const w = await mountList([issue({ last_seen: 'not-a-number' })])
    expect(w.findAll('[data-testid="backend-issue-row"]')).toHaveLength(1)
  })
})
