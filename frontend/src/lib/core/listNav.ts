// frontend/src/lib/listNav.js
// Pure "move by delta within a list" helper. Centralizes the "no-selection -> land on an end;
// else clamp" semantics that used to be inlined in LogTable.moveSelection — shared by any view
// that steps through a list with j/k or a peek-drawer's prev/next.

/**
 * Compute the next index after moving by `delta`, clamped to the list bounds.
 *
 * @param length - list length.
 * @param currentIndex - 0-based current index, or -1 if nothing is selected.
 * @param delta - step direction/size (positive = forward, negative = backward).
 * @returns the new index, clamped to [0, length-1]; -1 if the list is empty.
 */
export function nextIndex(length: number, currentIndex: number, delta: number): number {
  if (!length) return -1
  if (currentIndex === -1) return delta > 0 ? 0 : length - 1
  return Math.min(length - 1, Math.max(0, currentIndex + delta))
}
