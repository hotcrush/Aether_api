import { getCache, setCache } from './commands'

const CACHE_KEY = 'aether:quota_refresh_settings'

export const QUOTA_REFRESH_INTERVALS = [5, 15, 30, 60] as const
export type QuotaRefreshInterval = typeof QUOTA_REFRESH_INTERVALS[number]

export interface QuotaRefreshSettings {
  enabled: boolean
  intervalMinutes: QuotaRefreshInterval
}

export const DEFAULT_QUOTA_REFRESH_SETTINGS: QuotaRefreshSettings = {
  enabled: false,
  intervalMinutes: 15,
}

let writeQueue: Promise<void> = Promise.resolve()

export async function loadQuotaRefreshSettings(): Promise<QuotaRefreshSettings> {
  try {
    const raw = await getCache(CACHE_KEY)
    if (!raw) return DEFAULT_QUOTA_REFRESH_SETTINGS
    const parsed = JSON.parse(raw) as Partial<QuotaRefreshSettings>
    const intervalMinutes = QUOTA_REFRESH_INTERVALS.find(
      (value) => value === Number(parsed.intervalMinutes),
    )
    return {
      enabled: parsed.enabled === true,
      intervalMinutes: intervalMinutes ?? DEFAULT_QUOTA_REFRESH_SETTINGS.intervalMinutes,
    }
  } catch {
    return DEFAULT_QUOTA_REFRESH_SETTINGS
  }
}

export function saveQuotaRefreshSettings(settings: QuotaRefreshSettings) {
  const pending = writeQueue.then(async () => {
    try {
      await setCache(CACHE_KEY, JSON.stringify(settings))
    } catch {
      // Keep the in-memory preference when persistent storage is unavailable.
    }
  })
  writeQueue = pending.catch(() => undefined)
  return pending
}
