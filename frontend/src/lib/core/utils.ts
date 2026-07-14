import { type ClassValue, clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'

/**
 * Merge class values with clsx (conditional classes) then dedupe conflicting
 * Tailwind utilities with tailwind-merge. This is the `cn()` helper every ui
 * primitive and downstream component uses.
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs))
}
