import type { RelayUsageQueryState, RelayUsageSummary } from '../types'
import { getCache, setCache } from './commands'

const CACHE_KEY = 'aether:relay_usage_cache'
const TTL_MS = 60 * 60 * 1000 // 1 hour

interface CacheEntry {
  usage: RelayUsageSummary
  cached_at: number
}

type CacheMap = Record<string, CacheEntry>

async function readCache(): Promise<CacheMap> {
  try {
    const raw = await getCache(CACHE_KEY)
    return raw ? JSON.parse(raw) as CacheMap : {}
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

export async function loadRelayUsageCache(): Promise<Record<string, RelayUsageQueryState>> {
  const cache = await readCache()
  const now = Date.now()
  const fresh = Object.entries(cache).filter(([, entry]) => now - entry.cached_at <= TTL_MS)
  if (fresh.length !== Object.keys(cache).length) {
    await writeCache(Object.fromEntries(fresh))
  }
  return Object.fromEntries(
    fresh.map(([id, entry]) => [id, { status: 'success', usage: entry.usage }]),
  ) as Record<string, RelayUsageQueryState>
}

export async function saveRelayUsageToCache(accountId: string, usage: RelayUsageSummary) {
  const cache = await readCache()
  cache[accountId] = { usage, cached_at: Date.now() }
  await writeCache(cache)
}

export async function removeRelayUsageFromCache(accountId: string) {
  const cache = await readCache()
  if (!cache[accountId]) return
  delete cache[accountId]
  await writeCache(cache)
}
