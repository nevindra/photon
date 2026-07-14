import { describe, it, expect, vi, beforeEach } from 'vitest'
import { useCopy } from '@/lib/core/useCopy'

const toastSpy = vi.fn()
vi.mock('@/components/ui/toast', () => ({ useToast: () => ({ toast: toastSpy }) }))

describe('useCopy', () => {
  beforeEach(() => {
    toastSpy.mockClear()
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue() } })
  })
  it('writes text and toasts with the label', async () => {
    const { copy } = useCopy()
    await copy('abc123', 'trace ID')
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('abc123')
    expect(toastSpy).toHaveBeenCalledWith(expect.objectContaining({ title: 'Copied trace ID' }))
  })
})
