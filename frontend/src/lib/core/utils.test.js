import { describe, it, expect } from 'vitest'
import { cn } from '@/lib/core/utils'

describe('cn', () => {
  it('merges conflicting tailwind utilities, keeping the last', () => {
    expect(cn('p-2', 'p-4')).toBe('p-4')
  })

  it('drops falsy / conditional classes', () => {
    const active = false
    expect(cn('text-foreground', active && 'text-sev-error')).toBe('text-foreground')
  })

  it('keeps a truthy conditional class', () => {
    const active = true
    expect(cn('px-2', active && 'px-4')).toBe('px-4')
  })

  it('combines non-conflicting classes', () => {
    expect(cn('bg-background', 'text-foreground')).toBe('bg-background text-foreground')
  })
})
