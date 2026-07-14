// Module-level toast store (NOT per-component state). Every `useToast()` call — whether from a
// component's `<script setup>` or fired straight out of a TanStack Query mutation's
// onSuccess/onError (see lib/uptimeQueries.js, lib/usersQueries.js, lib/dataQueries.js) — reads
// and writes this same reactive queue, so the single `<Toaster />` mounted once at the app root
// (App.vue) renders every toast raised anywhere in the app.
import { ref } from 'vue'

export type ToastVariant = 'default' | 'success' | 'error' | 'warning'

export interface Toast {
  id: number
  open: boolean
  title?: string
  description?: string
  variant: ToastVariant
  duration: number
}

export interface ToastOptions {
  title?: string
  description?: string
  variant?: ToastVariant
  duration?: number
}

const DEFAULT_DURATION = 5000
// Only the most recent toasts stay queued — a burst of writes (e.g. a bulk action) drops the
// oldest rather than growing the viewport forever.
const MAX_TOASTS = 4

// Plain incrementing counter (not Date.now()/Math.random()) so ids are stable, collision-free,
// and sort in call order even if two toasts fire within the same millisecond.
let idCounter = 0

const toasts = ref<Toast[]>([])

/** Push a new toast onto the shared queue. Returns the created toast (handy in tests). */
export function toast(options: ToastOptions = {}): Toast {
  const t: Toast = {
    id: (idCounter += 1),
    open: true,
    title: options.title,
    description: options.description,
    variant: options.variant ?? 'default',
    duration: options.duration ?? DEFAULT_DURATION,
  }
  toasts.value = [...toasts.value, t].slice(-MAX_TOASTS)
  return t
}

// Marks the toast closed (so `<ToastRoot>` plays its exit animation via `data-state=closed`)
// then drops it from the queue once the animation has had time to finish.
export function dismiss(id: number) {
  const t = toasts.value.find((candidate) => candidate.id === id)
  if (!t || !t.open) return
  t.open = false
  setTimeout(() => {
    toasts.value = toasts.value.filter((candidate) => candidate.id !== id)
  }, 300)
}

export function useToast() {
  return { toasts, toast, dismiss }
}
