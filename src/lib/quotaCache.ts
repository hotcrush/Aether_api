import type { AccountQuota, QuotaQueryState } from '../types'
import { getCache, setCache } from './commands'

const CACHE_KEY = 'aether:quota_cache'
const TTL_MS = 60 * 60 * 1000 // 1 hour

interface CacheEntry {
  quota: AccountQuota
  cached_at: number
}

type CacheMap = Record<string, CacheEntry>

async function readCache(): Promise<CacheMap> {
  try {
    const raw = await getCache(CACHE_KEY)
    if (!raw) return {}
    return JSON.parse(raw) as CacheMap
  } catch {
    return {}
  }
}

async function writeCache(cache: CacheMap) {
  try {
    await setCache(CACHE_KEY, JSON.stringify(cache))
  } catch {
    // DB unavailable – silently ignore
  }
}

/**
 * Load cached quota states on app startup.
 * Entries older than TTL are discarded.
 */
export async function loadQuotaCache(): Promise<Record<string, QuotaQueryState>> {
  const cache = await readCache()
  const now = Date.now()
  const result: Record<string, QuotaQueryState> = {}
  let dirty = false

  for (const [id, entry] of Object.entries(cache)) {
    if (now - entry.cached_at > TTL_MS) {
      dirty = true
      continue
    }
    result[id] = { status: 'success', quota: entry.quota }
  }

  if (dirty) {
    const pruned: CacheMap = {}
    for (const [id, entry] of Object.entries(cache)) {
      if (now - entry.cached_at <= TTL_MS) {
        pruned[id] = entry
      }
    }
    await writeCache(pruned)
  }

  return result
}

/**
 * Persist a successful quota result to cache.
 */
export async function saveQuotaToCache(accountId: string, quota: AccountQuota) {
  const cache = await readCache()
  cache[accountId] = { quota, cached_at: Date.now() }
  await writeCache(cache)
}

/**
 * Remove a single account from cache (e.g. on delete).
 */
export async function removeQuotaFromCache(accountId: string) {
  const cache = await readCache()
  if (cache[accountId]) {
    delete cache[accountId]
    await writeCache(cache)
  }
}

/**
 * Clear all cached quota data.
 */
export async function clearQuotaCache() {
  try {
    await setCache(CACHE_KEY, '{}')
  } catch {
    // ignore
  }
}
