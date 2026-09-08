// Pure helpers behind `/settings/users` — table-testable, no Vue, no I/O.
import type { UserInfo } from '@/lib/core/api'
import type { Role } from '@/lib/deploy/roles'

/** Server limits mirrored from `photon-api/src/auth.rs` so the form can refuse before the round trip. */
export const MIN_PASSWORD_LEN = 8
export const MAX_USERNAME_LEN = 64

export interface PasswordErrors {
  password?: string
  /** The confirmation does not match — only ever set once the password itself is acceptable. */
  confirm?: string
  /** Changing your own password without saying the current one. */
  current?: string
}

export interface CredentialErrors extends PasswordErrors {
  username?: string
}

/**
 * The password rule the server applies, plus the one only the form can check: the admin typed it
 * twice the same. The mismatch is reported only once the password is long enough — a short one
 * has one problem, not two.
 */
export function validatePassword(password: string, confirm: string): PasswordErrors {
  const errors: PasswordErrors = {}
  if (password.length < MIN_PASSWORD_LEN) errors.password = `At least ${MIN_PASSWORD_LEN} characters.`
  else if (confirm !== password) errors.confirm = 'Passwords do not match.'
  return errors
}

/** The change-password form: the new password twice, and the current one when it is your own. */
export function validatePasswordChange(input: { password: string; confirm: string; current: string; requireCurrent: boolean }): PasswordErrors {
  const errors = validatePassword(input.password, input.confirm)
  if (input.requireCurrent && input.current === '') errors.current = 'Enter your current password.'
  return errors
}

/** The same checks the server applies, in the same words, so a rejection never surprises. */
export function validateCredentials(username: string, password: string, confirm: string = password): CredentialErrors {
  const errors: CredentialErrors = validatePassword(password, confirm)
  const u = username.trim()
  if (u === '') errors.username = 'Enter a username.'
  else if (u.length > MAX_USERNAME_LEN) errors.username = `At most ${MAX_USERNAME_LEN} characters.`
  else if (/\s/.test(u)) errors.username = 'No spaces — it is what they type to sign in.'
  return errors
}

/** A user whose role the server did not report is an admin — the era before roles had one. */
export function roleOf(u: Pick<UserInfo, 'role'>): Role {
  return u.role ?? 'admin'
}

export interface RoleCounts {
  admin: number
  operator: number
  viewer: number
}

export function countRoles(users: UserInfo[]): RoleCounts {
  const c: RoleCounts = { admin: 0, operator: 0, viewer: 0 }
  for (const u of users) c[roleOf(u)] += 1
  return c
}

/** Case-insensitive username substring, plus an optional exact-role narrowing. */
export function filterUsers(users: UserInfo[], q: string, role: Role | null): UserInfo[] {
  const needle = q.trim().toLowerCase()
  return users.filter((u) => (!needle || u.username.toLowerCase().includes(needle)) && (!role || roleOf(u) === role))
}

export const PAGE_SIZES = [10, 25, 50] as const
export const DEFAULT_PAGE_SIZE: (typeof PAGE_SIZES)[number] = 10

export interface Page<T> {
  rows: T[]
  /** 1-based, clamped into `1..=pages` — a page that emptied out (a removal, a narrower search) slides back. */
  page: number
  pages: number
  /** 1-based inclusive bounds of `rows` within the whole; both 0 when there is nothing. */
  from: number
  to: number
  total: number
}

/** One page of a list. Pure, so the clamping rule is one place and table-testable. */
export function paginate<T>(items: T[], page: number, size: number): Page<T> {
  const total = items.length
  const pages = Math.max(1, Math.ceil(total / Math.max(1, size)))
  const p = Math.min(Math.max(1, Math.floor(page) || 1), pages)
  const start = (p - 1) * size
  const rows = items.slice(start, start + size)
  return { rows, page: p, pages, from: total === 0 ? 0 : start + 1, to: start + rows.length, total }
}

/** Which page an item sits on (1-based), or null when it is not in the list. */
export function pageOf<T>(items: T[], predicate: (item: T) => boolean, size: number): number | null {
  const i = items.findIndex(predicate)
  return i < 0 ? null : Math.floor(i / size) + 1
}

/** Up to two initials for the avatar: "rina" → "RI", "al faris" → "AF". */
export function initials(username: string): string {
  const parts = username.trim().split(/[\s._-]+/).filter(Boolean)
  const s = parts.length >= 2 ? parts[0][0] + parts[1][0] : (parts[0] ?? '?').slice(0, 2)
  return s.toUpperCase()
}

/** "today" / "yesterday" / "12 days ago" / "3 months ago" / "2 years ago", from a ms timestamp. */
export function addedLabel(createdMs: number, nowMs: number = Date.now()): string {
  // `created_at` of 0 is the seeded first account — it has no honest date.
  if (!createdMs) return 'at setup'
  const days = Math.floor((nowMs - createdMs) / 86_400_000)
  if (days <= 0) return 'today'
  if (days === 1) return 'yesterday'
  if (days < 30) return `${days} days ago`
  const months = Math.floor(days / 30)
  if (months < 12) return months === 1 ? 'a month ago' : `${months} months ago`
  const years = Math.floor(days / 365)
  return years === 1 ? 'a year ago' : `${years} years ago`
}

/** Why this row's Remove button is disabled, or null when it is allowed. Pure so the reasons are one list. */
export function removeBlockedReason(
  u: Pick<UserInfo, 'username'>,
  ctx: { me: string | null; total: number; allowed: boolean },
): string | null {
  if (!ctx.allowed) return 'Requires the Admin role'
  if (u.username === ctx.me) return 'You cannot remove your own account'
  if (ctx.total <= 1) return 'The last remaining user cannot be removed'
  return null
}

/** Why this row's "Change password" is disabled, or null. Your own is always yours to change; anyone else's needs the Users permission. */
export function passwordBlockedReason(u: Pick<UserInfo, 'username'>, ctx: { me: string | null; allowed: boolean }): string | null {
  if (u.username === ctx.me || ctx.allowed) return null
  return 'Requires the Admin role'
}
