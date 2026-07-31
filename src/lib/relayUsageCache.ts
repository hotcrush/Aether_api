import type { RelayUsageQueryState, RelayUsageSummary } from '../types'
import { getCache, setCache } from './commands'

const CACHE_KEY = 'aether:relay_usage_cache'
const FRESH_TTL_MS = 60 * 60 * 1000 // 1 hour

interface CacheEntry {
  usage?: RelayUsageSummary
  error?: string
  cached_at: number
  last_attempt_at?: number
}

type CacheMap = Record<string, CacheEntry>

export interface RelayUsageCacheSnapshot {
  states: Record<string, RelayUsageQueryState>
  staleAccountIds: string[]
}

let mutationQueue: Promise<void> = Promise.resolve()

function enqueueMutation(operation: () => Promise<void>) {
  const pending = mutationQueue.then(operation)
  mutationQueue = pending.catch(() => undefined)
  return pending
}

async function readCache(): Promise<CacheMap | null> {
  let raw: string | null
  try {
    raw = await getCache(CACHE_KEY)
  } catch {
    return null
  }
  if (!raw) return {}

  try {
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {}

    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, CacheEntry] => {
        const value = entry[1]
        return Boolean(
          value
          && typeof value === 'object'
          && typeof (value as CacheEntry).cached_at === 'number'
          && Number.isFinite((value as CacheEntry).cached_at)
          && (
            Boolean((value as CacheEntry).usage && typeof (value as CacheEntry).usage === 'object')
            || typeof (value as CacheEntry).error === 'string'
          ),
        )
      }),
    )
  } catch {
    return {}
  }
}

async function writeCache(cache: CacheMap) {
  try {
    await setCache(CACHE_KEY, JSON.stringify(cache))
  } catch {
    // The live result remains usable even when persistent storage is unavailable.
  }
}

/** Restore every last-known value and separately report entries due for refresh. */
export async function loadRelayUsageCache(): Promise<RelayUsageCacheSnapshot> {
  await mutationQueue
  const cache = await readCache() ?? {}
  const now = Date.now()
  const states: Record<string, RelayUsageQueryState> = {}
  const staleAccountIds: string[] = []

  for (const [id, entry] of Object.entries(cache)) {
    states[id] = entry.usage
      ? { status: 'success', usage: entry.usage }
      : { status: 'error', error: entry.error || '上次用量读取失败' }
    const freshnessAt = entry.last_attempt_at ?? entry.cached_at
    if (now - freshnessAt > FRESH_TTL_MS) staleAccountIds.push(id)
  }

  return { states, staleAccountIds }
}

export function saveRelayUsageToCache(accountId: string, usage: RelayUsageSummary) {
  return enqueueMutation(async () => {
    const cache = await readCache()
    if (!cache) return
    const cachedAt = Date.now()
    cache[accountId] = { usage, cached_at: cachedAt, last_attempt_at: cachedAt }
    await writeCache(cache)
  })
}

/** Remember a failed attempt so unsupported relay sites are not retried on every restart. */
export function saveRelayUsageFailureToCache(accountId: string, error: string) {
  return enqueueMutation(async () => {
    const cache = await readCache()
    if (!cache) return
    const attemptedAt = Date.now()
    const previous = cache[accountId]
    cache[accountId] = previous?.usage
      ? { ...previous, error, last_attempt_at: attemptedAt }
      : { error, cached_at: attemptedAt, last_attempt_at: attemptedAt }
    await writeCache(cache)
  })
}

export function removeRelayUsageFromCache(accountId: string) {
  return enqueueMutation(async () => {
    const cache = await readCache()
    if (!cache) return
    if (!cache[accountId]) return
    delete cache[accountId]
    await writeCache(cache)
  })
}
