import { describe, it, expect } from 'vitest'
import { validateCredentials, validatePassword, validatePasswordChange, countRoles, filterUsers, initials, addedLabel, removeBlockedReason, passwordBlockedReason, paginate, pageOf } from './users'

const DAY = 86_400_000

describe('validateCredentials', () => {
  it('passes a trimmed name and an 8-character password', () => {
    expect(validateCredentials('  rina ', 'hunter22')).toEqual({})
  })
  it('names the failing field in the server’s terms', () => {
    expect(validateCredentials('', 'short')).toEqual({ username: 'Enter a username.', password: 'At least 8 characters.' })
    expect(validateCredentials('a b', 'longenough').username).toMatch(/spaces/)
    expect(validateCredentials('x'.repeat(65), 'longenough').username).toMatch(/64/)
  })
  it('wants the password typed the same twice, but only once it is long enough', () => {
    expect(validateCredentials('rina', 'longenough', 'longenough')).toEqual({})
    expect(validateCredentials('rina', 'longenough', 'longenougk')).toEqual({ confirm: 'Passwords do not match.' })
    expect(validateCredentials('rina', 'short', 'different')).toEqual({ password: 'At least 8 characters.' })
    expect(validatePassword('longenough', '')).toEqual({ confirm: 'Passwords do not match.' })
  })
})

describe('validatePasswordChange', () => {
  it('asks for the current password only when it is your own', () => {
    const base = { password: 'longenough', confirm: 'longenough', current: '' }
    expect(validatePasswordChange({ ...base, requireCurrent: false })).toEqual({})
    expect(validatePasswordChange({ ...base, requireCurrent: true })).toEqual({ current: 'Enter your current password.' })
    expect(validatePasswordChange({ ...base, current: 'old', requireCurrent: true })).toEqual({})
  })
})

describe('paginate / pageOf', () => {
  const items = Array.from({ length: 23 }, (_, i) => `u${i + 1}`)
  it('slices a page and reports its bounds', () => {
    expect(paginate(items, 1, 10)).toMatchObject({ page: 1, pages: 3, from: 1, to: 10, total: 23 })
    expect(paginate(items, 3, 10)).toMatchObject({ page: 3, pages: 3, from: 21, to: 23, rows: ['u21', 'u22', 'u23'] })
  })
  it('clamps a page that no longer exists, and an empty list to a single empty page', () => {
    expect(paginate(items, 9, 10)).toMatchObject({ page: 3, from: 21 })
    expect(paginate(items, 0, 10).page).toBe(1)
    expect(paginate([], 4, 10)).toEqual({ rows: [], page: 1, pages: 1, from: 0, to: 0, total: 0 })
  })
  it('finds the page an item sits on', () => {
    expect(pageOf(items, (u) => u === 'u11', 10)).toBe(2)
    expect(pageOf(items, (u) => u === 'nope', 10)).toBeNull()
  })
})

describe('countRoles / filterUsers', () => {
  const users = [
    { username: 'demo', created_at: 0, role: 'admin' as const },
    { username: 'Rina', created_at: 0, role: 'operator' as const },
    { username: 'legacy', created_at: 0 }, // a pre-roles backend reports none: that is an admin
  ]
  it('reads a missing role as admin', () => {
    expect(countRoles(users)).toEqual({ admin: 2, operator: 1, viewer: 0 })
  })
  it('matches case-insensitively and narrows by role', () => {
    expect(filterUsers(users, 'RIN', null).map((u) => u.username)).toEqual(['Rina'])
    expect(filterUsers(users, '', 'admin').map((u) => u.username)).toEqual(['demo', 'legacy'])
    expect(filterUsers(users, 'demo', 'viewer')).toEqual([])
  })
})

describe('initials', () => {
  it('takes two letters, or one from each of two words', () => {
    expect(initials('rina')).toBe('RI')
    expect(initials('al.faris')).toBe('AF')
    expect(initials('x')).toBe('X')
  })
})

describe('addedLabel', () => {
  const now = 1_800_000_000_000
  it('speaks in days, then months, then years', () => {
    expect(addedLabel(now, now)).toBe('today')
    expect(addedLabel(now - DAY, now)).toBe('yesterday')
    expect(addedLabel(now - 12 * DAY, now)).toBe('12 days ago')
    expect(addedLabel(now - 45 * DAY, now)).toBe('a month ago')
    expect(addedLabel(now - 400 * DAY, now)).toBe('a year ago')
  })
  it('does not invent a date for the seeded first account', () => {
    expect(addedLabel(0, now)).toBe('at setup')
  })
})

describe('removeBlockedReason', () => {
  it('refuses in the order the server would', () => {
    expect(removeBlockedReason({ username: 'b' }, { me: 'a', total: 2, allowed: false })).toMatch(/Admin role/)
    expect(removeBlockedReason({ username: 'a' }, { me: 'a', total: 2, allowed: true })).toMatch(/own account/)
    expect(removeBlockedReason({ username: 'b' }, { me: 'a', total: 1, allowed: true })).toMatch(/last remaining/)
    expect(removeBlockedReason({ username: 'b' }, { me: 'a', total: 2, allowed: true })).toBeNull()
  })
})

describe('passwordBlockedReason', () => {
  it('lets anyone change their own, and an admin anyone’s', () => {
    expect(passwordBlockedReason({ username: 'a' }, { me: 'a', allowed: false })).toBeNull()
    expect(passwordBlockedReason({ username: 'b' }, { me: 'a', allowed: false })).toMatch(/Admin role/)
    expect(passwordBlockedReason({ username: 'b' }, { me: 'a', allowed: true })).toBeNull()
  })
})
