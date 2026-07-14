import { ref, watch, type Ref } from 'vue'

// Per-table column state (built-in show/hide + added attribute columns),
// persisted to localStorage keyed by tableKey ('spans' | 'traces' | 'logs').
//
// Exposed surface (consumed by the trace + spans tables, and re-consumed by logs):
//   visibleKeys  Ref<string[]>  built-in column keys currently shown, in builtins order
//   attrColumns  Ref<string[]>  user-added attribute column keys, in insertion order
//   isVisible(key)   -> boolean  is a built-in currently shown
//   toggleBuiltin(key)          show/hide a built-in column
//   addAttr(key)                add an attribute column (idempotent)
//   removeAttr(key)             drop an attribute column
//
// All builtins start visible by default (there is no partial-default seeding) — every consumer
// wants the full built-in set shown on first load, so there is nothing for a `defaults` param to
// customize; it was accepted but never actually affected seeding and has been removed.

// A built-in column descriptor. Callers (TraceTable/SpanTable) attach extra display-only fields
// (label, width, ...) that this composable never reads — only `key` is load-bearing here.
export interface TableColumnDef {
  key: string
  [extra: string]: unknown
}

export interface UseTableColumnsOptions {
  builtins: TableColumnDef[]
}

export interface UseTableColumnsReturn {
  visibleKeys: Ref<string[]>
  attrColumns: Ref<string[]>
  isVisible: (key: string) => boolean
  toggleBuiltin: (key: string) => void
  addAttr: (key: string) => void
  removeAttr: (key: string) => void
}

// Shape persisted to localStorage under `photon.cols.<tableKey>`.
interface StoredColumnState {
  hidden: string[]
  attrs: string[]
}

export function useTableColumns(tableKey: string, { builtins }: UseTableColumnsOptions): UseTableColumnsReturn {
  const storeKey = 'photon.cols.' + tableKey
  const seed = load(storeKey) ?? { hidden: [], attrs: [] }
  const hidden = ref(new Set(seed.hidden))
  const attrColumns = ref([...seed.attrs])
  const visibleKeys = ref(builtins.map((b) => b.key).filter((k) => !hidden.value.has(k)))

  function persist() {
    localStorage.setItem(storeKey, JSON.stringify({ hidden: [...hidden.value], attrs: attrColumns.value }))
  }
  // flush: 'sync' so a mutation is durable immediately — a second useTableColumns(tableKey)
  // constructed in the same tick (and outside any component, so no render flush occurs) reads
  // the just-written state back from localStorage.
  watch([hidden, attrColumns], persist, { deep: true, flush: 'sync' })

  return {
    visibleKeys,
    attrColumns,
    isVisible: (k: string) => !hidden.value.has(k),
    toggleBuiltin(k: string) {
      hidden.value.has(k) ? hidden.value.delete(k) : hidden.value.add(k)
      hidden.value = new Set(hidden.value)
      visibleKeys.value = builtins.map((b) => b.key).filter((x) => !hidden.value.has(x))
    },
    addAttr(k: string) {
      if (!attrColumns.value.includes(k)) attrColumns.value = [...attrColumns.value, k]
    },
    removeAttr(k: string) {
      attrColumns.value = attrColumns.value.filter((x) => x !== k)
    },
  }
}

function load(k: string): StoredColumnState | null {
  try {
    return JSON.parse(localStorage.getItem(k) as string) as StoredColumnState
  } catch {
    return null
  }
}
