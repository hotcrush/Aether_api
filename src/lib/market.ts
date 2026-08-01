import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  isPermissionGranted,
  requestPermission,
} from '@tauri-apps/plugin-notification'

export type MarketStatus = 'idle' | 'loading' | 'online' | 'partial' | 'error'
export type MarketRange = '24h' | '7d' | '30d'
export type MarketCategory = 'k12' | 'gptplus' | 'bugteam'
export type MarketSeverity = 'success' | 'info' | 'medium' | 'warning' | 'high' | string
export type MarketNotificationPermission = 'checking' | 'granted' | 'prompt' | 'denied' | 'unavailable'

export interface MarketProduct {
  id: string
  goodsKey: string
  shopToken: string
  shopName: string
  shopUrl: string
  name: string
  url: string
  price: number
  fee: number
  feeRate: number
  feePayer: number
  totalPrice: number
  marketPrice: number
  stockCount: number
  sourceCategory: string
  category: MarketCategory | string | null
  matchTerms: string[]
  verificationStatus: 'verified' | 'unverified' | 'unknown' | string
  missingCount: number
  firstSeenAt: string
  lastSeenAt: string
}

export interface MarketShop {
  platform: string
  token: string
  fallbackName: string
  name: string
  enabled: boolean
  ok: boolean
  error: string | null
  failureCount: number
  blockedUntil: string | null
  feeRate: number
  feePayer: number
  feeCheckedAt: string | null
  profileCheckedAt: string | null
  lastCheckedAt: string | null
  lastSuccessAt: string | null
  goodsTypes: string[]
  productCount: number
  totalStock: number
}

export interface MarketProtection {
  mode: 'normal' | 'backoff' | 'circuit-open' | string
  consecutiveFailures: number
  circuitOpenUntil: string | null
  circuitReason: string | null
  lastAttemptAt: string | null
  lastRequestAt: string | null
  lastSuccessAt: string | null
  requestTimestamps: string[]
  activeApiBase: string
  fallbackUsed: boolean
  dataMode: 'live' | 'cached' | 'empty' | string
}

export interface MarketSnapshot {
  status: MarketStatus | string
  products: MarketProduct[]
  shops: MarketShop[]
  protection: MarketProtection
  lastCheckedAt: string | null
  nextRefreshAt: string | null
  unreadAlertCount: number
}

export interface MarketCategoryMetric {
  key: MarketCategory | string
  label: string
  totalStock: number
  weightedAveragePrice: number
  minimumPrice: number
  productCount: number
}

export interface MarketAnalyticsPoint {
  capturedAt: string
  totalStock: number
  productCount: number
  categories: MarketCategoryMetric[]
}

export interface MarketEvent {
  seq: number
  eventId: string
  kind: string
  entityType: 'product' | 'store' | 'market' | string
  entityId: string
  occurredAt: string
  expiresAt: string | null
  severity: MarketSeverity
  title: string
  body: string
  section: 'products' | 'stores' | 'analytics' | string
  payload: Record<string, unknown>
  readAt: string | null
  notifiedAt: string | null
}

export interface MarketAnalyticsSnapshot {
  range: MarketRange
  generatedAt: string
  points: MarketAnalyticsPoint[]
  events: MarketEvent[]
  totalSamples: number
}

export interface MarketAlertSettings {
  enabled: boolean
  nativeEnabled: boolean
  bugTeamAvailable: boolean
  productUnavailable: boolean
  storeHealth: boolean
  stockSurge: boolean
  priceOutlier: boolean
  quietHoursEnabled: boolean
  quietHoursStart: string
  quietHoursEnd: string
}

export interface MarketShopInput {
  token: string
  fallbackName: string
  platform: 'liandx' | string
  enabled: boolean
}

export interface MarketRefreshResult {
  performed: boolean
  reason: string | null
  retryAt: string | null
  message: string | null
  snapshot: MarketSnapshot
}

export const DEFAULT_MARKET_ALERT_SETTINGS: MarketAlertSettings = {
  enabled: true,
  nativeEnabled: true,
  bugTeamAvailable: true,
  productUnavailable: false,
  storeHealth: true,
  stockSurge: false,
  priceOutlier: false,
  quietHoursEnabled: false,
  quietHoursStart: '23:00',
  quietHoursEnd: '08:00',
}

export const getMarketSnapshot = () =>
  invoke<MarketSnapshot>('get_market_snapshot')

export const refreshMarket = () =>
  invoke<MarketRefreshResult>('refresh_market')

export const getMarketAnalytics = (range: MarketRange) =>
  invoke<MarketAnalyticsSnapshot>('get_market_analytics', { range })

export const listMarketAlerts = (limit = 200) =>
  invoke<MarketEvent[]>('list_market_alerts', { limit })

export const markMarketAlertsRead = (eventIds?: string[]) =>
  invoke<number>('mark_market_alerts_read', { eventIds })

export const getMarketAlertSettings = () =>
  invoke<MarketAlertSettings>('get_market_alert_settings')

export const updateMarketAlertSettings = (settings: MarketAlertSettings) =>
  invoke<MarketAlertSettings>('update_market_alert_settings', { settings })

export const upsertMarketShop = (input: MarketShopInput) =>
  invoke<MarketSnapshot>('upsert_market_shop', { input })

export const setMarketShopEnabled = (token: string, enabled: boolean) =>
  invoke<MarketSnapshot>('set_market_shop_enabled', { token, enabled })

export const deleteMarketShop = (token: string) =>
  invoke<MarketSnapshot>('delete_market_shop', { token })

export async function getMarketNotificationPermission(): Promise<MarketNotificationPermission> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return 'unavailable'
  try {
    return await isPermissionGranted() ? 'granted' : 'prompt'
  } catch {
    return 'unavailable'
  }
}

export async function requestMarketNotificationPermission(): Promise<MarketNotificationPermission> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return 'unavailable'
  try {
    const permission = await requestPermission()
    if (permission === 'granted') return 'granted'
    if (permission === 'denied') return 'denied'
    return 'prompt'
  } catch {
    return 'unavailable'
  }
}

export function listenMarketSnapshot(
  handler: (snapshot: MarketSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<MarketSnapshot>('market:snapshot', ({ payload }) => handler(payload))
}

export function listenMarketAlert(
  handler: (event: MarketEvent) => void,
): Promise<UnlistenFn> {
  return listen<MarketEvent>('market:alert', ({ payload }) => handler(payload))
}
