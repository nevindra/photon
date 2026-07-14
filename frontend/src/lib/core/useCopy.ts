import { useToast } from '@/components/ui/toast'

// Clipboard write + a "Copied …" toast. Replaces the bare navigator.clipboard
// writes scattered across the trace detail views (which gave no feedback).
export function useCopy() {
  const { toast } = useToast()
  async function copy(text: any, label?: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(String(text))
      toast({ title: `Copied ${label ?? 'to clipboard'}` })
    } catch {
      toast({ title: 'Copy failed', variant: 'error' })
    }
  }
  return { copy }
}

export interface UseCopyReturn {
  copy: (text: any, label?: string) => Promise<void>
}
