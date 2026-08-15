import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  isPermissionGranted,
  requestPermission,
} from '@tauri-apps/plugin-notification'

const previewMode = import.meta.env.DEV
  && typeof window !== 'undefined'
  && new URLSearchParams(window.location.search).has('preview')

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

export interface MarketRefreshProgress {
  completed: number
  total: number
  shopToken: string | null
  shopName: string | null
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

function previewMarketSnapshot(): MarketSnapshot {
  return {
    status: 'online',
    products: [
      {
        id: 'p1', goodsKey: 'gpt-plus-1', shopToken: 'shop-a', shopName: 'AI\u5c0f\u94fa', shopUrl: 'https://shop-a.example.com',
        name: 'ChatGPT Plus \u6210\u54c1\u53f7', url: '', price: 168, fee: 2.5, feeRate: 0.015, feePayer: 1,
        totalPrice: 170.5, marketPrice: 175, stockCount: 42, sourceCategory: 'gptplus', category: 'gptplus',
        matchTerms: ['plus'], verificationStatus: 'verified', missingCount: 0,
        firstSeenAt: '2026-07-20T08:00:00Z', lastSeenAt: '2026-08-01T06:30:00Z',
      },
      {
        id: 'p2', goodsKey: 'team-key-2', shopToken: 'shop-b', shopName: '\u6570\u7801\u5546\u57ce', shopUrl: 'https://shop-b.example.com',
        name: 'Codex Team \u5e74\u4ed8\u8d26\u53f7', url: '', price: 520, fee: 7.8, feeRate: 0.015, feePayer: 1,
        totalPrice: 527.8, marketPrice: 540, stockCount: 8, sourceCategory: 'bugteam', category: 'bugteam',
        matchTerms: ['team'], verificationStatus: 'verified', missingCount: 0,
        firstSeenAt: '2026-07-25T10:00:00Z', lastSeenAt: '2026-08-01T05:45:00Z',
      },
      {
        id: 'p3', goodsKey: 'k12-pack', shopToken: 'shop-a', shopName: 'AI\u5c0f\u94fa', shopUrl: 'https://shop-a.example.com',
        name: 'K12 \u6559\u80b2\u5957\u9910', url: '', price: 89, fee: 1.3, feeRate: 0.015, feePayer: 1,
        totalPrice: 90.3, marketPrice: 95, stockCount: 120, sourceCategory: 'k12', category: 'k12',
        matchTerms: ['k12'], verificationStatus: 'unverified', missingCount: 1,
        firstSeenAt: '2026-07-28T14:00:00Z', lastSeenAt: '2026-08-01T06:00:00Z',
      },
    ],
    shops: [
      {
        platform: 'liandx', token: 'shop-a', fallbackName: 'AI\u5c0f\u94fa', name: 'AI\u5c0f\u94fa',
        enabled: true, ok: true, error: null, failureCount: 0, blockedUntil: null,
        feeRate: 0.015, feePayer: 1, feeCheckedAt: '2026-08-01T04:00:00Z',
        profileCheckedAt: '2026-08-01T04:00:00Z', lastCheckedAt: '2026-08-01T06:30:00Z',
        lastSuccessAt: '2026-08-01T06:30:00Z', goodsTypes: ['gptplus', 'k12'], productCount: 2, totalStock: 162,
      },
      {
        platform: 'liandx', token: 'shop-b', fallbackName: '\u6570\u7801\u5546\u57ce', name: '\u6570\u7801\u5546\u57ce',
        enabled: true, ok: true, error: null, failureCount: 0, blockedUntil: null,
        feeRate: 0.015, feePayer: 1, feeCheckedAt: '2026-08-01T03:00:00Z',
        profileCheckedAt: '2026-08-01T03:00:00Z', lastCheckedAt: '2026-08-01T05:45:00Z',
        lastSuccessAt: '2026-08-01T05:45:00Z', goodsTypes: ['bugteam'], productCount: 1, totalStock: 8,
      },
    ],
    protection: {
      mode: 'normal', consecutiveFailures: 0, circuitOpenUntil: null, circuitReason: null,
      lastAttemptAt: '2026-08-01T06:30:00Z', lastRequestAt: '2026-08-01T06:30:00Z',
      lastSuccessAt: '2026-08-01T06:30:00Z', requestTimestamps: [], activeApiBase: 'https://api.example.com',
      fallbackUsed: false, dataMode: 'live',
    },
    lastCheckedAt: '2026-08-01T06:30:00Z',
    nextRefreshAt: null,
    unreadAlertCount: 2,
  }
}

function previewMarketAnalytics(range: MarketRange): MarketAnalyticsSnapshot {
  const now = Date.now()
  const points: MarketAnalyticsPoint[] = Array.from({ length: 12 }, (_, i) => ({
    capturedAt: new Date(now - (11 - i) * 3_600_000).toISOString(),
    totalStock: 150 + Math.round(Math.sin(i * 0.8) * 30) + i * 2,
    productCount: 3,
    categories: [
      { key: 'gptplus', label: 'GPT Plus', totalStock: 42 + i, weightedAveragePrice: 168 - i * 0.3, minimumPrice: 155, productCount: 1 },
      { key: 'bugteam', label: 'Team', totalStock: 8, weightedAveragePrice: 520, minimumPrice: 510, productCount: 1 },
      { key: 'k12', label: 'K12', totalStock: 100 + i * 2, weightedAveragePrice: 89, minimumPrice: 85, productCount: 1 },
    ],
  }))
  const events: MarketEvent[] = [
    {
      seq: 2, eventId: 'evt-preview-2', kind: 'stock_surge', entityType: 'product', entityId: 'p3',
      occurredAt: new Date(now - 7_200_000).toISOString(), expiresAt: null, severity: 'info',
      title: '\u5e93\u5b58\u6fc0\u589e', body: 'K12 \u6559\u80b2\u5957\u9910 \u5e93\u5b58\u4ece 80 \u589e\u81f3 120',
      section: 'products', payload: {}, readAt: null, notifiedAt: null,
    },
    {
      seq: 1, eventId: 'evt-preview-1', kind: 'price_drop', entityType: 'product', entityId: 'p1',
      occurredAt: new Date(now - 14_400_000).toISOString(), expiresAt: null, severity: 'success',
      title: '\u4ef7\u683c\u4e0b\u964d', body: 'ChatGPT Plus \u6210\u54c1\u53f7 \u4ef7\u683c\u4ece \u00a5175 \u964d\u81f3 \u00a5168',
      section: 'products', payload: {}, readAt: null, notifiedAt: null,
    },
  ]
  return { range, generatedAt: new Date(now).toISOString(), points, events, totalSamples: 168 }
}

export const getMarketSnapshot = () =>
  previewMode
    ? Promise.resolve(previewMarketSnapshot())
    : invoke<MarketSnapshot>('get_market_snapshot')

export const refreshMarket = () =>
  previewMode
    ? Promise.resolve({ performed: true, reason: null, retryAt: null, message: null, snapshot: previewMarketSnapshot() } as MarketRefreshResult)
    : invoke<MarketRefreshResult>('refresh_market')

export const getMarketAnalytics = (range: MarketRange) =>
  previewMode
    ? Promise.resolve(previewMarketAnalytics(range))
    : invoke<MarketAnalyticsSnapshot>('get_market_analytics', { range })

export const listMarketAlerts = (limit = 200) =>
  previewMode
    ? Promise.resolve(previewMarketAnalytics('24h').events.slice(0, limit))
    : invoke<MarketEvent[]>('list_market_alerts', { limit })

export const markMarketAlertsRead = (eventIds?: string[]) =>
  previewMode ? Promise.resolve(eventIds?.length ?? 2) : invoke<number>('mark_market_alerts_read', { eventIds })

export const getMarketAlertSettings = () =>
  previewMode ? Promise.resolve({ ...DEFAULT_MARKET_ALERT_SETTINGS }) : invoke<MarketAlertSettings>('get_market_alert_settings')

export const updateMarketAlertSettings = (settings: MarketAlertSettings) =>
  previewMode ? Promise.resolve(settings) : invoke<MarketAlertSettings>('update_market_alert_settings', { settings })

export const upsertMarketShop = (input: MarketShopInput) =>
  previewMode ? Promise.resolve(previewMarketSnapshot()) : invoke<MarketSnapshot>('upsert_market_shop', { input })

export const setMarketShopEnabled = (token: string, enabled: boolean) =>
  previewMode ? Promise.resolve(previewMarketSnapshot()) : invoke<MarketSnapshot>('set_market_shop_enabled', { token, enabled })

export const deleteMarketShop = (token: string) =>
  previewMode ? Promise.resolve(previewMarketSnapshot()) : invoke<MarketSnapshot>('delete_market_shop', { token })

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
  if (previewMode) return Promise.resolve(() => {})
  return listen<MarketSnapshot>('market:snapshot', ({ payload }) => handler(payload))
}

export function listenMarketAlert(
  handler: (event: MarketEvent) => void,
): Promise<UnlistenFn> {
  if (previewMode) return Promise.resolve(() => {})
  return listen<MarketEvent>('market:alert', ({ payload }) => handler(payload))
}

export function listenMarketRefreshProgress(
  handler: (progress: MarketRefreshProgress) => void,
): Promise<UnlistenFn> {
  if (previewMode) return Promise.resolve(() => {})
  return listen<MarketRefreshProgress>('market:refresh-progress', ({ payload }) => handler(payload))
}
