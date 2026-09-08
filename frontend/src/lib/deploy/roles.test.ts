import { describe, it, expect } from 'vitest'
import { roleAllows, permissionHint, requiredRole, isRole, ROLES } from './roles'

describe('roles', () => {
  it('ranks viewer < operator < admin over every permission', () => {
    expect(roleAllows('viewer', 'audit')).toBe(true)
    expect(roleAllows('viewer', 'lifecycle')).toBe(false)
    expect(roleAllows('operator', 'lifecycle')).toBe(true)
    expect(roleAllows('operator', 'deploy')).toBe(true)
    expect(roleAllows('operator', 'templates')).toBe(false)
    expect(roleAllows('operator', 'exec')).toBe(false)
    expect(roleAllows('admin', 'exec')).toBe(true)
    expect(roleAllows('admin', 'users')).toBe(true)
  })

  it('denies everything to no role at all', () => {
    expect(roleAllows(null, 'audit')).toBe(false)
    expect(roleAllows(undefined, 'lifecycle')).toBe(false)
  })

  it('names the role a disabled control needs', () => {
    expect(permissionHint('lifecycle')).toBe('Requires the Operator role')
    expect(permissionHint('agents')).toBe('Requires the Admin role')
    expect(requiredRole('compose')).toBe('admin')
  })

  it('recognises only the three roles', () => {
    for (const r of ROLES) expect(isRole(r)).toBe(true)
    expect(isRole('root')).toBe(false)
    expect(isRole(null)).toBe(false)
  })
})
