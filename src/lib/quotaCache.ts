import type { AccountQuota, QuotaQueryState } from '../types'
import { getCache, mergeCacheEntries, setCache } from './commands'

const CACHE_KEY = 'aether:quota_cache'
const FRESH_TTL_MS = 60 * 60 * 1000 // 1 hour

interface CacheEntry {
  quota: AccountQuota
  cached_at: number
}

type CacheMap = Record<string, CacheEntry>

export interface QuotaCacheSnapshot {
  states: Record<string, QuotaQueryState>
  staleAccountIds: string[]
}

export interface QuotaCacheUpdate {
  accountId: string
  quota: AccountQuota
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
          && (value as CacheEntry).quota
          && typeof (value as CacheEntry).quota === 'object',
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
export async function loadQuotaCache(): Promise<QuotaCacheSnapshot> {
  await mutationQueue
  const cache = await readCache() ?? {}
  const now = Date.now()
  const states: Record<string, QuotaQueryState> = {}
  const staleAccountIds: string[] = []

  for (const [id, entry] of Object.entries(cache)) {
    states[id] = { status: 'success', quota: entry.quota }
    if (now - entry.cached_at > FRESH_TTL_MS) staleAccountIds.push(id)
  }

  return { states, staleAccountIds }
}

/** Persist one successful quota result without racing other cache updates. */
export function saveQuotaToCache(accountId: string, quota: AccountQuota) {
  return saveQuotaBatchToCache([{ accountId, quota }])
}

/** Merge a complete query batch with one read and one write. */
export function saveQuotaBatchToCache(updates: readonly QuotaCacheUpdate[]) {
  if (!updates.length) return Promise.resolve()
  return enqueueMutation(async () => {
    const cachedAt = Date.now()
    const entries: Record<string, CacheEntry> = {}
    for (const { accountId, quota } of updates) {
      entries[accountId] = { quota, cached_at: cachedAt }
    }
    try {
      await mergeCacheEntries(CACHE_KEY, entries)
    } catch {
      // Keep the live result even when persistent storage is unavailable.
    }
  })
}

/** Remove a single account from cache (e.g. on delete). */
export function removeQuotaFromCache(accountId: string) {
  return enqueueMutation(async () => {
    const cache = await readCache()
    if (!cache) return
    if (!cache[accountId]) return
    delete cache[accountId]
    await writeCache(cache)
  })
}

/** Clear all cached quota data in sequence with pending updates. */
export function clearQuotaCache() {
  return enqueueMutation(() => writeCache({}))
}
