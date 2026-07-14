// Favorites + recent metrics for the picker. localStorage-backed with an in-memory fallback
// (private mode / quota). Storage is injected so tests hand in a fake.
import { ref, type Ref } from 'vue'

const FAV_KEY = 'photon.metrics.favorites'
const RECENT_KEY = 'photon.metrics.recent'
const RECENT_CAP = 8

export interface MetricFavorites {
  favorites: Ref<string[]>
  recent: Ref<string[]>
  isFavorite: (name: string) => boolean
  toggleFavorite: (name: string) => void
  recordRecent: (name: string) => void
}

function readList(storage: Storage | null, key: string): string[] {
  try {
    const raw = storage?.getItem(key)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
}
function writeList(storage: Storage | null, key: string, list: string[]): void {
  try {
    storage?.setItem(key, JSON.stringify(list))
  } catch {
    /* blocked / quota — keep the in-memory ref only */
  }
}

const defaultStorage = (): Storage | null =>
  typeof window !== 'undefined' ? window.localStorage : null

export function createMetricFavorites(storage: Storage | null = defaultStorage()): MetricFavorites {
  const favorites = ref<string[]>(readList(storage, FAV_KEY))
  const recent = ref<string[]>(readList(storage, RECENT_KEY))

  const isFavorite = (name: string): boolean => favorites.value.includes(name)

  function toggleFavorite(name: string): void {
    if (!name) return
    favorites.value = isFavorite(name)
      ? favorites.value.filter((n) => n !== name)
      : [...favorites.value, name]
    writeList(storage, FAV_KEY, favorites.value)
  }

  function recordRecent(name: string): void {
    if (!name) return
    recent.value = [name, ...recent.value.filter((n) => n !== name)].slice(0, RECENT_CAP)
    writeList(storage, RECENT_KEY, recent.value)
  }

  return { favorites, recent, isFavorite, toggleFavorite, recordRecent }
}
