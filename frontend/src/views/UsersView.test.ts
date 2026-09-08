// `/settings/users`: the table with its per-row rules, the search + role filter, paging, the add
// modal's inline validation and submit, the two password shapes, and the remove confirmation. The api layer is mocked; auth is
// seeded as an admin on a server that speaks roles.
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory } from 'vue-router'
import { VueQueryPlugin, QueryClient } from '@tanstack/vue-query'
import { TooltipProvider } from '@/components/ui/tooltip'
import { _setHydratedForTest } from '@/lib/core/auth'
import UsersView from './UsersView.vue'

const DAY = 86_400_000
let users: { username: string; created_at: number; role: 'viewer' | 'operator' | 'admin' }[]

vi.mock('@/lib/core/api', () => ({
  api: {
    mock: false,
    tenants: vi.fn().mockResolvedValue({ tenants: [] }),
    listUsers: vi.fn().mockImplementation(async () => ({ users })),
    createUser: vi.fn().mockImplementation(async (username: string, _p: string, role: 'viewer' | 'operator' | 'admin') => {
      if (users.some((u) => u.username === username)) return { ok: false, error: 'user already exists' }
      users = [...users, { username, created_at: Date.now(), role }]
      return { ok: true }
    }),
    deleteUser: vi.fn().mockImplementation(async (username: string) => {
      users = users.filter((u) => u.username !== username)
      return { ok: true }
    }),
    updateUserRole: vi.fn().mockImplementation(async (username: string, role: 'viewer' | 'operator' | 'admin') => {
      users = users.map((u) => (u.username === username ? { ...u, role } : u))
      return { ok: true }
    }),
    changePassword: vi.fn().mockImplementation(async (_username: string, _password: string, currentPassword?: string) => {
      if (currentPassword !== undefined && currentPassword !== 'hunter22') return { ok: false, error: 'current password is incorrect' }
      return { ok: true }
    }),
  },
}))

const routes = [
  { path: '/settings/users', component: { template: '<div />' } },
  { path: '/login', component: { template: '<div />' } },
]
function queryPlugin(): [typeof VueQueryPlugin, { queryClient: QueryClient }] {
  return [VueQueryPlugin, { queryClient: new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false } } }) }]
}
async function mountView() {
  const router = createRouter({ history: createMemoryHistory(), routes })
  router.push('/settings/users')
  await router.isReady()
  const wrapper = mount(
    { components: { TooltipProvider, UsersView }, template: '<TooltipProvider><UsersView /></TooltipProvider>' },
    { global: { plugins: [router, queryPlugin()] }, attachTo: document.body },
  )
  await flushPromises()
  return { wrapper, router }
}
const rowNames = (w: ReturnType<typeof mount>) => w.findAll('[data-testid="user-row"]').map((r) => r.attributes('data-username'))
// Dialogs portal to <body>, so they are reached through the document, not the wrapper.
const inDialog = (testid: string) => document.querySelector(`[data-testid="${testid}"]`) as HTMLElement | null
async function setDialogValue(testid: string, value: string): Promise<void> {
  const el = inDialog(testid) as HTMLInputElement
  el.value = value
  el.dispatchEvent(new Event('input', { bubbles: true }))
  await flushPromises()
}
async function submitDialog(formId: string): Promise<void> {
  ;(document.getElementById(formId) as HTMLFormElement).dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
  await flushPromises()
}

describe('UsersView', () => {
  beforeEach(() => {
    window.history.replaceState(null, '', '/settings/users')
    document.body.innerHTML = ''
    users = [
      { username: 'admin', created_at: 0, role: 'admin' },
      { username: 'rina', created_at: Date.now() - 12 * DAY, role: 'operator' },
      { username: 'bayu', created_at: Date.now() - DAY, role: 'viewer' },
    ]
    _setHydratedForTest({ authenticated: true, username: 'admin', needs_setup: false, role: 'admin' })
  })

  it('lists everyone with a role, an age, and the anti-lockout rules on Remove', async () => {
    const { wrapper } = await mountView()
    expect(rowNames(wrapper)).toEqual(['admin', 'rina', 'bayu'])
    expect(wrapper.find('[data-testid="users-summary"]').text()).toMatch(/3\s*users/)
    expect(wrapper.find('[data-testid="users-summary"]').text()).toMatch(/1\s*admin/)
    const me = wrapper.find('[data-username="admin"]')
    expect(me.find('[data-testid="user-you"]').exists()).toBe(true)
    // Your own role is a badge, not a picker; everyone else's is a picker for an admin.
    expect(me.find('[data-testid="role-admin"]').attributes('data-role')).toBe('admin')
    expect(wrapper.find('[data-username="rina"] [aria-label="Role for rina"]').text()).toBe('Operator')
    expect(wrapper.find('[data-username="bayu"]').text()).toContain('yesterday')
    expect(me.text()).toContain('at setup')
    expect((wrapper.find('[data-testid="remove-admin"]').element as HTMLButtonElement).disabled).toBe(true)
    expect((wrapper.find('[data-testid="remove-rina"]').element as HTMLButtonElement).disabled).toBe(false)
    // One admin among several users: the lockout warning is up.
    expect(wrapper.find('[data-testid="sole-admin-note"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('narrows by name and by role, and says how many are hidden', async () => {
    const { wrapper } = await mountView()
    await wrapper.find('[data-testid="users-search"]').setValue('RI')
    expect(rowNames(wrapper)).toEqual(['rina'])
    expect(wrapper.find('[data-testid="users-filter-count"]').text()).toBe('Showing 1 of 3')
    await wrapper.find('[data-testid="users-search"]').setValue('zzz')
    expect(wrapper.find('[data-testid="users-empty"]').text()).toContain('No one matches')
    wrapper.unmount()
  })

  it('opens a modal, validates inline, needs the password twice, then adds and closes', async () => {
    const { wrapper } = await mountView()
    const { api } = await import('@/lib/core/api')
    expect(inDialog('add-user-dialog')).toBeNull()
    await wrapper.find('[data-testid="add-user-open"]').trigger('click')
    await flushPromises()
    expect(inDialog('add-user-dialog')).not.toBeNull()

    await submitDialog('add-user-form')
    expect(api.createUser).not.toHaveBeenCalled()
    expect(inDialog('add-user-dialog')?.textContent).toContain('Enter a username.')

    await setDialogValue('new-username', 'sari')
    await setDialogValue('new-password', 'short')
    // The first submit already flagged the field, so the rule shows as the error row, not the hint.
    expect(inDialog('add-user-dialog')?.textContent).toContain('At least 8 characters.')
    expect(inDialog('password-hint')).toBeNull()
    await setDialogValue('new-password', 'longenough')
    expect(inDialog('password-hint')?.textContent).toContain('Long enough')
    // Typed once is not enough: the confirmation has to match before anything is sent.
    await submitDialog('add-user-form')
    expect(api.createUser).not.toHaveBeenCalled()
    expect(inDialog('add-user-dialog')?.textContent).toContain('Passwords do not match.')
    await setDialogValue('confirm-password', 'longenough')
    expect(inDialog('confirm-hint')?.textContent).toContain('Matches')
    ;(inDialog('role-option-operator')?.querySelector('input') as HTMLInputElement).click()
    await submitDialog('add-user-form')
    expect(api.createUser).toHaveBeenCalledWith('sari', 'longenough', 'operator')
    expect(rowNames(wrapper)).toContain('sari')
    expect(wrapper.find('[data-username="sari"]').attributes('data-state')).toBe('selected')
    // Done: the modal is gone, and opening it again starts from a blank form.
    expect(inDialog('add-user-dialog')).toBeNull()
    await wrapper.find('[data-testid="add-user-open"]').trigger('click')
    await flushPromises()
    expect((inDialog('new-username') as HTMLInputElement).value).toBe('')
    wrapper.unmount()
  })

  it('puts a duplicate-name refusal under the username field', async () => {
    const { wrapper } = await mountView()
    await wrapper.find('[data-testid="add-user-open"]').trigger('click')
    await flushPromises()
    await setDialogValue('new-username', 'rina')
    await setDialogValue('new-password', 'longenough')
    await setDialogValue('confirm-password', 'longenough')
    await submitDialog('add-user-form')
    expect(inDialog('add-user-dialog')?.textContent).toContain('user already exists')
    expect(inDialog('add-user-error')).toBeNull()
    wrapper.unmount()
  })

  it('changes your own password only with the current one, and resets another’s without it', async () => {
    const { wrapper } = await mountView()
    const { api } = await import('@/lib/core/api')
    await wrapper.find('[data-testid="password-admin"]').trigger('click')
    await flushPromises()
    expect(inDialog('change-password-dialog')?.textContent).toContain('Change your password')
    await setDialogValue('current-password', 'wrong')
    await setDialogValue('new-password', 'newpass99')
    await setDialogValue('confirm-password', 'newpass99')
    await submitDialog('change-password-form')
    expect(api.changePassword).toHaveBeenCalledWith('admin', 'newpass99', 'wrong')
    // The refusal lands under the field it is about, and the dialog stays open.
    expect(inDialog('change-password-dialog')?.textContent).toContain('current password is incorrect')
    await setDialogValue('current-password', 'hunter22')
    await submitDialog('change-password-form')
    expect(api.changePassword).toHaveBeenLastCalledWith('admin', 'newpass99', 'hunter22')
    expect(inDialog('change-password-dialog')).toBeNull()

    // An admin resets someone else: no current password asked.
    await wrapper.find('[data-testid="password-rina"]').trigger('click')
    await flushPromises()
    expect(inDialog('change-password-dialog')?.textContent).toContain('Reset password for rina')
    expect(inDialog('current-password')).toBeNull()
    await setDialogValue('new-password', 'rinapass9')
    await setDialogValue('confirm-password', 'rinapass9')
    await submitDialog('change-password-form')
    expect(api.changePassword).toHaveBeenLastCalledWith('rina', 'rinapass9', undefined)
    expect(inDialog('change-password-dialog')).toBeNull()
    wrapper.unmount()
  })

  it('pages a long roster, resets to the first page on a search, and turns to the new user’s page', async () => {
    users = Array.from({ length: 23 }, (_, i) => ({ username: `user${String(i + 1).padStart(2, '0')}`, created_at: Date.now(), role: 'viewer' as const }))
    users[0] = { ...users[0], username: 'admin', role: 'admin' }
    const { wrapper } = await mountView()
    expect(rowNames(wrapper)).toHaveLength(10)
    expect(wrapper.find('[data-testid="pagination-range"]').text()).toBe('1–10 of 23 users')
    expect((wrapper.find('[data-testid="pagination-prev"]').element as HTMLButtonElement).disabled).toBe(true)
    await wrapper.find('[data-testid="pagination-next"]').trigger('click')
    await wrapper.find('[data-testid="pagination-next"]').trigger('click')
    expect(wrapper.find('[data-testid="pagination-page"]').text()).toBe('Page 3 of 3')
    expect(rowNames(wrapper)).toEqual(['user21', 'user22', 'user23'])
    expect((wrapper.find('[data-testid="pagination-next"]').element as HTMLButtonElement).disabled).toBe(true)
    // A search starts over from page one.
    await wrapper.find('[data-testid="users-search"]').setValue('user2')
    expect(wrapper.find('[data-testid="pagination-page"]').text()).toBe('Page 1 of 1')
    expect(wrapper.find('[data-testid="pagination-range"]').text()).toBe('1–4 of 4 matching')
    await wrapper.find('[data-testid="users-search"]').setValue('')
    // A new user sorts to the end (the mock appends): the table turns to their page.
    await wrapper.find('[data-testid="add-user-open"]').trigger('click')
    await flushPromises()
    await setDialogValue('new-username', 'zed')
    await setDialogValue('new-password', 'longenough')
    await setDialogValue('confirm-password', 'longenough')
    await submitDialog('add-user-form')
    expect(wrapper.find('[data-testid="pagination-page"]').text()).toBe('Page 3 of 3')
    expect(wrapper.find('[data-username="zed"]').attributes('data-state')).toBe('selected')
    wrapper.unmount()
  })

  it('confirms before removing, then removes', async () => {
    const { wrapper } = await mountView()
    const { api } = await import('@/lib/core/api')
    await wrapper.find('[data-testid="remove-bayu"]').trigger('click')
    await flushPromises()
    const dialog = document.querySelector('[data-testid="remove-user-dialog"]')
    expect(dialog?.textContent).toContain('Remove bayu?')
    ;(document.querySelector('[data-testid="remove-user-confirm"]') as HTMLButtonElement).click()
    await flushPromises()
    expect(api.deleteUser).toHaveBeenCalledWith('bayu')
    expect(rowNames(wrapper)).toEqual(['admin', 'rina'])
    wrapper.unmount()
  })

  it('gates Add and the others’ passwords for a viewer, who may still change their own', async () => {
    _setHydratedForTest({ authenticated: true, username: 'bayu', needs_setup: false, role: 'viewer' })
    const { wrapper } = await mountView()
    expect((wrapper.find('[data-testid="add-user-open"]').element as HTMLButtonElement).disabled).toBe(true)
    expect(wrapper.find('[data-testid="roles-card"]').exists()).toBe(true)
    // No role pickers for a viewer — badges only — and Remove is disabled everywhere.
    expect(wrapper.find('[data-testid="role-rina"]').attributes('data-role')).toBe('operator')
    expect((wrapper.find('[data-testid="remove-rina"]').element as HTMLButtonElement).disabled).toBe(true)
    expect((wrapper.find('[data-testid="password-rina"]').element as HTMLButtonElement).disabled).toBe(true)
    expect((wrapper.find('[data-testid="password-bayu"]').element as HTMLButtonElement).disabled).toBe(false)
    wrapper.unmount()
  })
})
