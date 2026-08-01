import { getCache, setCache } from './commands'

const CACHE_KEY = 'aether:daily_budget_usd'

export async function loadDailyBudget() {
  const raw = await getCache(CACHE_KEY)
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as { limitUsd?: unknown } | null
    const value = parsed?.limitUsd
    return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null
  } catch {
    return null
  }
}

export function saveDailyBudget(limitUsd: number | null) {
  return setCache(CACHE_KEY, JSON.stringify({ limitUsd }))
}
