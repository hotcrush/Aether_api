import {
  AlertCircle,
  BarChart3,
  Bell,
  BellRing,
  Boxes,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  PackageSearch,
  Pencil,
  Plus,
  RefreshCw,
  Scale,
  Search,
  Store,
  Trash2,
} from 'lucide-react'
import {
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react'
import { errorText } from '../lib/format'
import { formatDateTime, formatShortDate } from '../lib/time'
import type { OpenWebWorkspaceTabInput } from '../lib/workspaceTabs'
import {
  DEFAULT_MARKET_ALERT_SETTINGS,
  deleteMarketShop,
  getMarketAlertSettings,
  getMarketAnalytics,
  getMarketNotificationPermission,
  getMarketSnapshot,
  listenMarketAlert,
  listenMarketSnapshot,
  listMarketAlerts,
  markMarketAlertsRead,
  refreshMarket,
  requestMarketNotificationPermission,
  setMarketShopEnabled,
  updateMarketAlertSettings,
  upsertMarketShop,
  type MarketAlertSettings,
  type MarketAnalyticsPoint,
  type MarketAnalyticsSnapshot,
  type MarketEvent,
  type MarketNotificationPermission,
  type MarketProduct,
  type MarketRange,
  type MarketShop,
  type MarketShopInput,
  type MarketSnapshot,
} from '../lib/market'
import { Dialog } from './Dialog'

export type MarketSection = 'products' | 'stores' | 'analytics' | 'alerts'

type ProductView = 'compare' | 'stores' | 'all'
type ProductPriceScope = 'bargain' | 'affordable' | 'all'
type VerificationFilter = 'all' | 'verified' | 'unverified' | 'unknown'
type CategoryFilter = 'all' | 'focus' | 'k12' | 'gptplus' | 'bugteam' | 'other'

interface MarketMonitorPageProps {
  initialSection?: MarketSection
  onUnreadCountChange?: (count: number) => void
  onSectionChange?: (section: MarketSection) => void
  onOpenWebPage: (input: OpenWebWorkspaceTabInput) => void
}

interface ShopEditorState {
  originalToken: string | null
  input: MarketShopInput
}

interface ProductPriceProfile {
  median: number
  bargain: number
  affordableCeiling: number
}

type ProductPriceTier = 'bargain' | 'affordable' | 'high'

const PRODUCT_PAGE_SIZE = 30
const ALERT_PAGE_SIZE = 15

const sections: Array<{ id: MarketSection; label: string; icon: ReactNode }> = [
  { id: 'products', label: '商品比价', icon: <PackageSearch size={15} /> },
  { id: 'stores', label: '店铺状态', icon: <Store size={15} /> },
  { id: 'analytics', label: '行情分析', icon: <BarChart3 size={15} /> },
  { id: 'alerts', label: '提醒', icon: <Bell size={15} /> },
]

const categoryLabels: Record<string, string> = {
  focus: '★ 关注',
  k12: 'K12',
  gptplus: 'GPT Plus',
  bugteam: 'BUG TEAM',
  other: '其他',
}

const FOCUS_CATEGORIES = new Set(['k12', 'gptplus', 'bugteam'])

export function MarketMonitorPage({
  initialSection = 'products',
  onUnreadCountChange,
  onSectionChange,
  onOpenWebPage,
}: MarketMonitorPageProps) {
  const [section, setSection] = useState<MarketSection>(initialSection)
  const [snapshot, setSnapshot] = useState<MarketSnapshot | null>(null)
  const [alerts, setAlerts] = useState<MarketEvent[]>([])
  const [settings, setSettings] = useState<MarketAlertSettings>(DEFAULT_MARKET_ALERT_SETTINGS)
  const [savedSettings, setSavedSettings] = useState<MarketAlertSettings>(DEFAULT_MARKET_ALERT_SETTINGS)
  const [range, setRange] = useState<MarketRange>('24h')
  const [analytics, setAnalytics] = useState<MarketAnalyticsSnapshot | null>(null)
  const [loading, setLoading] = useState(true)
  const [analyticsLoading, setAnalyticsLoading] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [settingsSaving, setSettingsSaving] = useState(false)
  const [notificationPermission, setNotificationPermission] = useState<MarketNotificationPermission>('checking')
  const [permissionRequesting, setPermissionRequesting] = useState(false)
  const [busyShops, setBusyShops] = useState<Set<string>>(() => new Set())
  const [error, setError] = useState('')
  const [analyticsError, setAnalyticsError] = useState('')
  const [notice, setNotice] = useState('')
  const [shopEditor, setShopEditor] = useState<ShopEditorState | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<MarketShop | null>(null)

  useEffect(() => {
    setSection(initialSection)
  }, [initialSection])

  useEffect(() => {
    let disposed = false
    void getMarketSnapshot()
      .then((value) => {
        if (!disposed) {
          setSnapshot(value)
          setError('')
        }
      })
      .catch((nextError) => { if (!disposed) setError(errorText(nextError)) })
      .finally(() => { if (!disposed) setLoading(false) })

    void getMarketAlertSettings()
      .then((value) => {
        if (!disposed) {
          setSettings(value)
          setSavedSettings(value)
        }
      })
      .catch((nextError) => { if (!disposed) setError(errorText(nextError)) })

    void listMarketAlerts()
      .then((value) => { if (!disposed) setAlerts(value) })
      .catch((nextError) => { if (!disposed) setError(errorText(nextError)) })

    let stopSnapshot: (() => void) | undefined
    let stopAlert: (() => void) | undefined
    void listenMarketSnapshot((value) => { if (!disposed) setSnapshot(value) })
      .then((stop) => disposed ? stop() : (stopSnapshot = stop))
      .catch(() => undefined)
    void listenMarketAlert((event) => {
      if (disposed) return
      setAlerts((current) => [event, ...current.filter((item) => item.eventId !== event.eventId)].slice(0, 200))
    })
      .then((stop) => disposed ? stop() : (stopAlert = stop))
      .catch(() => undefined)

    return () => {
      disposed = true
      stopSnapshot?.()
      stopAlert?.()
    }
  }, [])

  useEffect(() => {
    let disposed = false
    void getMarketNotificationPermission().then((permission) => {
      if (!disposed) setNotificationPermission(permission)
    })
    return () => { disposed = true }
  }, [])

  useEffect(() => {
    onUnreadCountChange?.(snapshot?.unreadAlertCount ?? 0)
  }, [onUnreadCountChange, snapshot?.unreadAlertCount])

  const loadAnalytics = useCallback(async () => {
    setAnalyticsLoading(true)
    try {
      setAnalytics(await getMarketAnalytics(range))
      setAnalyticsError('')
    } catch (nextError) {
      setAnalyticsError(errorText(nextError))
    } finally {
      setAnalyticsLoading(false)
    }
  }, [range])

  useEffect(() => {
    if (section === 'analytics') void loadAnalytics()
  }, [section, snapshot?.lastCheckedAt, loadAnalytics])

  const refresh = async () => {
    if (refreshing) return
    setRefreshing(true)
    setNotice('')
    try {
      const result = await refreshMarket()
      setSnapshot(result.snapshot)
      setError('')
      setNotice(result.performed
        ? `已更新 ${result.snapshot.products.length} 件商品`
        : result.message || '刷新请求已延后')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setRefreshing(false)
    }
  }

  const setShopBusy = (token: string, busy: boolean) => {
    setBusyShops((current) => {
      const next = new Set(current)
      if (busy) next.add(token)
      else next.delete(token)
      return next
    })
  }

  const openNewShop = () => {
    setShopEditor({
      originalToken: null,
      input: { token: '', fallbackName: '', platform: 'liandx', enabled: true },
    })
  }

  const openShopEditor = (shop: MarketShop) => {
    setShopEditor({
      originalToken: shop.token,
      input: {
        token: shop.token,
        fallbackName: shop.fallbackName || shop.name,
        platform: shop.platform,
        enabled: shop.enabled,
      },
    })
  }

  const saveShop = async () => {
    if (!shopEditor) return
    const input = {
      ...shopEditor.input,
      token: shopEditor.input.token.trim(),
      fallbackName: shopEditor.input.fallbackName.trim(),
    }
    if (!input.token || !input.fallbackName) {
      setError('请填写店铺名称和 token')
      return
    }
    const busyKey = shopEditor.originalToken || input.token || 'new'
    setShopBusy(busyKey, true)
    try {
      setSnapshot(await upsertMarketShop(input))
      setShopEditor(null)
      setError('')
      setNotice(shopEditor.originalToken ? '店铺配置已更新' : '店铺已添加')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setShopBusy(busyKey, false)
    }
  }

  const toggleShop = async (shop: MarketShop) => {
    setShopBusy(shop.token, true)
    try {
      setSnapshot(await setMarketShopEnabled(shop.token, !shop.enabled))
      setError('')
      setNotice(shop.enabled ? '店铺监控已暂停' : '店铺监控已启用')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setShopBusy(shop.token, false)
    }
  }

  const removeShop = async () => {
    if (!deleteTarget) return
    setShopBusy(deleteTarget.token, true)
    try {
      setSnapshot(await deleteMarketShop(deleteTarget.token))
      setDeleteTarget(null)
      setError('')
      setNotice('店铺已删除')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setShopBusy(deleteTarget.token, false)
    }
  }

  const saveAlertSettings = async () => {
    setSettingsSaving(true)
    try {
      const value = await updateMarketAlertSettings(settings)
      setSettings(value)
      setSavedSettings(value)
      setError('')
      setNotice('提醒规则已保存')
    } catch (nextError) {
      setError(errorText(nextError))
    } finally {
      setSettingsSaving(false)
    }
  }

  const requestNotificationAccess = async () => {
    if (permissionRequesting) return
    setPermissionRequesting(true)
    const permission = await requestMarketNotificationPermission()
    setNotificationPermission(permission)
    if (permission === 'granted') {
      setSettings((current) => ({ ...current, nativeEnabled: true }))
      setNotice('系统通知已授权，请保存提醒规则')
    }
    setPermissionRequesting(false)
  }

  const markAlertsRead = async (ids?: string[]) => {
    try {
      await markMarketAlertsRead(ids)
      const readAt = new Date().toISOString()
      const selected = ids ? new Set(ids) : null
      setAlerts((current) => current.map((item) => (
        !item.readAt && (!selected || selected.has(item.eventId)) ? { ...item, readAt } : item
      )))
      setSnapshot((current) => current ? {
        ...current,
        unreadAlertCount: ids
          ? Math.max(0, current.unreadAlertCount - ids.length)
          : 0,
      } : current)
    } catch (nextError) {
      setError(errorText(nextError))
    }
  }

  const products = snapshot?.products ?? []
  const shops = snapshot?.shops ?? []
  const openProduct = (product: MarketProduct) => {
    onOpenWebPage({
      url: product.url,
      title: product.name || product.shopName || '商品',
      reuseKey: `market-product:${product.id}`,
      source: { kind: 'market', id: product.id },
    })
  }
  const openShop = (shop: MarketShop) => {
    const knownShopUrl = products.find(
      (product) => product.shopToken === shop.token && product.shopUrl.trim(),
    )?.shopUrl
    onOpenWebPage({
      url: knownShopUrl || `https://pay.ldxp.cn/shop/${encodeURIComponent(shop.token)}`,
      title: shop.name || shop.fallbackName || '店铺',
      reuseKey: `market-shop:${shop.token}`,
      source: { kind: 'market', id: shop.token },
    })
  }
  const openAlert = (event: MarketEvent) => {
    const url = payloadText(event.payload, 'url')
    if (!url) return
    const product = event.entityType === 'product'
      ? products.find((item) => item.id === event.entityId)
      : undefined
    const reuseKey = event.entityType === 'product' && event.entityId
      ? `market-product:${event.entityId}`
      : `market-alert:${event.entityType}:${event.entityId || event.eventId}`
    onOpenWebPage({
      url,
      title: product?.name || event.title || '市场详情',
      reuseKey,
      source: { kind: 'market', id: event.entityId || event.eventId },
    })
  }
  const enabledShops = shops.filter((shop) => shop.enabled)
  const onlineShops = enabledShops.filter((shop) => shop.ok)
  const settingsDirty = JSON.stringify(settings) !== JSON.stringify(savedSettings)

  return (
    <main className="market-page">
      <section className="market-dashboard" aria-label="市场监控">
        <header className="market-hero">
          <div className="market-hero-copy">
            <h2>市场监控</h2>
            <p>
              手动刷新 · {' '}
              {snapshot?.lastCheckedAt
                ? `更新于 ${formatDateTime(snapshot.lastCheckedAt)}`
                : loading ? '正在载入' : '等待首次采集'}
            </p>
          </div>
          <button className="btn market-refresh" onClick={() => { void refresh() }} disabled={refreshing}>
            <RefreshCw className={refreshing ? 'spin' : undefined} size={15} />
            {refreshing ? '刷新中' : '立即刷新'}
          </button>
        </header>

        {error && (
          <div className="market-feedback market-feedback-error" role="alert">
            <AlertCircle size={15} />
            <span>{error}</span>
            <button type="button" onClick={() => setError('')}>关闭</button>
          </div>
        )}
        {notice && (
          <div className="market-feedback" role="status">
            <CheckCircle2 size={15} />
            <span>{notice}</span>
            <button type="button" onClick={() => setNotice('')}>关闭</button>
          </div>
        )}

        <div className="market-summary">
          <MarketSummary label="有货商品" value={formatNumber(products.length)} />
          <MarketSummary
            label="在线店铺"
            value={`${onlineShops.length}/${enabledShops.length}`}
            tone={enabledShops.length === onlineShops.length ? 'healthy' : 'warning'}
          />
          <MarketSummary
            label="采集保护"
            value={protectionLabel(snapshot)}
            tone={snapshot?.protection.mode === 'normal' ? 'healthy' : 'warning'}
          />
          <MarketSummary
            label="未读提醒"
            value={formatNumber(snapshot?.unreadAlertCount ?? 0)}
            tone={(snapshot?.unreadAlertCount ?? 0) > 0 ? 'danger' : 'neutral'}
          />
        </div>

        <nav className="market-section-tabs" aria-label="市场监控视图">
          {sections.map((item) => (
            <button
              className={`market-section-tab${section === item.id ? ' active' : ''}`}
              aria-current={section === item.id ? 'page' : undefined}
              onClick={() => {
                setSection(item.id)
                onSectionChange?.(item.id)
              }}
              type="button"
              key={item.id}
            >
              {item.icon}
              <span>{item.label}</span>
              {item.id === 'alerts' && (snapshot?.unreadAlertCount ?? 0) > 0 && (
                <small>{Math.min(99, snapshot?.unreadAlertCount ?? 0)}</small>
              )}
            </button>
          ))}
        </nav>

        {section === 'products' && (
          <ProductSection products={products} shops={shops} onOpen={openProduct} />
        )}
        {section === 'stores' && (
          <StoreSection
            shops={shops}
            busyShops={busyShops}
            onAdd={openNewShop}
            onEdit={openShopEditor}
            onToggle={(shop) => { void toggleShop(shop) }}
            onDelete={setDeleteTarget}
            onOpen={openShop}
          />
        )}
        {section === 'analytics' && (
          <AnalyticsSection
            data={analytics}
            loading={analyticsLoading}
            error={analyticsError}
            range={range}
            onRangeChange={setRange}
            onRetry={() => { void loadAnalytics() }}
          />
        )}
        {section === 'alerts' && (
          <AlertsSection
            alerts={alerts}
            settings={settings}
            settingsDirty={settingsDirty}
            saving={settingsSaving}
            notificationPermission={notificationPermission}
            permissionRequesting={permissionRequesting}
            onSettingsChange={setSettings}
            onSave={() => { void saveAlertSettings() }}
            onRequestPermission={() => { void requestNotificationAccess() }}
            onMarkRead={(ids) => { void markAlertsRead(ids) }}
            onOpen={openAlert}
          />
        )}
      </section>

      <ShopEditorDialog
        editor={shopEditor}
        busy={Boolean(shopEditor && busyShops.has(shopEditor.originalToken || shopEditor.input.token || 'new'))}
        onChange={setShopEditor}
        onClose={() => setShopEditor(null)}
        onSave={() => { void saveShop() }}
      />
      <DeleteShopDialog
        shop={deleteTarget}
        busy={Boolean(deleteTarget && busyShops.has(deleteTarget.token))}
        onClose={() => setDeleteTarget(null)}
        onConfirm={() => { void removeShop() }}
      />
    </main>
  )
}

function MarketSummary({
  label,
  value,
  tone = 'neutral',
}: {
  label: string
  value: string
  tone?: 'healthy' | 'warning' | 'danger' | 'neutral'
}) {
  return (
    <div className={`market-summary-item market-tone-${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  )
}

function ProductSection({
  products,
  shops,
  onOpen,
}: {
  products: MarketProduct[]
  shops: MarketShop[]
  onOpen: (product: MarketProduct) => void
}) {
  const [query, setQuery] = useState('')
  const deferredQuery = useDeferredValue(query.trim().toLocaleLowerCase())
  const [category, setCategory] = useState<CategoryFilter>('focus')
  const [shopToken, setShopToken] = useState('all')
  const [minimumPrice, setMinimumPrice] = useState('')
  const [maximumPrice, setMaximumPrice] = useState('')
  const [view, setView] = useState<ProductView>('compare')
  const [priceScope, setPriceScope] = useState<ProductPriceScope>('all')
  const [verificationFilter, setVerificationFilter] = useState<VerificationFilter>('all')
  const [page, setPage] = useState(1)

  const priceProfiles = useMemo(() => buildProductPriceProfiles(products), [products])

  const filtered = useMemo(() => {
    const minimum = minimumPrice === '' ? null : Number(minimumPrice)
    const maximum = maximumPrice === '' ? null : Number(maximumPrice)
    return products
      .filter((product) => product.stockCount > 0)
      .filter((product) => {
        if (category === 'all') return true
        if (category === 'focus') return FOCUS_CATEGORIES.has(product.category || '')
        return (product.category || 'other') === category
      })
      .filter((product) => shopToken === 'all' || product.shopToken === shopToken)
      .filter((product) => category !== 'gptplus'
        || verificationFilter === 'all'
        || product.verificationStatus === verificationFilter)
      .filter((product) => matchesPriceScope(product, priceScope, priceProfiles))
      .filter((product) => minimum === null || !Number.isFinite(minimum) || product.totalPrice >= minimum)
      .filter((product) => maximum === null || !Number.isFinite(maximum) || product.totalPrice <= maximum)
      .filter((product) => !deferredQuery || [
        product.name,
        product.shopName,
        product.sourceCategory,
        product.matchTerms.join(' '),
      ].some((value) => value.toLocaleLowerCase().includes(deferredQuery)))
      .sort((left, right) => left.totalPrice - right.totalPrice || right.stockCount - left.stockCount)
  }, [
    category,
    deferredQuery,
    maximumPrice,
    minimumPrice,
    priceProfiles,
    priceScope,
    products,
    shopToken,
    verificationFilter,
  ])

  // 按商品名称去重：同名商品仅保留最低价，记录报价数与汇总库存
  const shopCountByName = useMemo(() => {
    const map = new Map<string, { shops: number; stock: number }>()
    for (const product of filtered) {
      const key = product.name.trim().toLocaleLowerCase()
      const entry = map.get(key)
      if (entry) {
        entry.shops += 1
        entry.stock += product.stockCount
      } else {
        map.set(key, { shops: 1, stock: product.stockCount })
      }
    }
    return map
  }, [filtered])

  const visible = useMemo(() => {
    const seen = new Set<string>()
    return filtered.filter((product) => {
      const key = product.name.trim().toLocaleLowerCase()
      if (seen.has(key)) return false
      seen.add(key)
      return true
    })
  }, [filtered])

  const totalPages = Math.max(1, Math.ceil(visible.length / PRODUCT_PAGE_SIZE))
  const currentPage = Math.min(page, totalPages)
  const pageOffset = (currentPage - 1) * PRODUCT_PAGE_SIZE
  const pageProducts = useMemo(
    () => visible.slice(pageOffset, pageOffset + PRODUCT_PAGE_SIZE),
    [pageOffset, visible],
  )
  const pageFirst = visible.length ? pageOffset + 1 : 0
  const pageLast = visible.length ? pageOffset + pageProducts.length : 0

  useEffect(() => {
    setPage(1)
  }, [
    category,
    maximumPrice,
    minimumPrice,
    priceScope,
    query,
    shopToken,
    verificationFilter,
    view,
  ])

  useEffect(() => {
    setPage((current) => Math.min(Math.max(1, current), totalPages))
  }, [totalPages])

  const groups = useMemo(() => {
    if (!pageProducts.length) return []
    if (view === 'all') {
      return [{ key: 'all', title: '全部有货', items: pageProducts }]
    }

    const grouped = new Map<string, MarketProduct[]>()
    for (const product of pageProducts) {
      const key = view === 'compare' ? product.category || 'other' : product.shopToken
      const group = grouped.get(key)
      if (group) group.push(product)
      else grouped.set(key, [product])
    }

    return [...grouped.entries()]
      .sort(([left, leftItems], [right, rightItems]) => view === 'compare'
        ? categoryOrder(left) - categoryOrder(right)
        : (leftItems[0]?.shopName || left).localeCompare(rightItems[0]?.shopName || right, 'zh-CN'))
      .map(([key, items]) => ({
        key,
        title: view === 'compare' ? categoryLabel(key) : items[0]?.shopName || key,
        items,
      }))
  }, [pageProducts, view])

  return (
    <section className="market-workspace market-products" aria-label="商品比价">
      <div className="market-product-toolbar">
        <div className="market-category-chips" aria-label="快捷分类筛选">
          {(['focus', 'k12', 'gptplus', 'bugteam', 'all'] as const).map((value) => (
            <button
              type="button"
              className={`market-category-chip${category === value ? ' active' : ''}`}
              onClick={() => setCategory(value)}
              key={value}
            >
              {categoryLabel(value)}
              {value !== 'all' && value !== 'focus' && (
                <small>{products.filter((p) => p.stockCount > 0 && (p.category || 'other') === value).length}</small>
              )}
              {value === 'focus' && (
                <small>{products.filter((p) => p.stockCount > 0 && FOCUS_CATEGORIES.has(p.category || '')).length}</small>
              )}
            </button>
          ))}
        </div>
        <div className="market-view-tabs" aria-label="商品浏览方式">
          <button type="button" className={view === 'compare' ? 'active' : ''} onClick={() => setView('compare')}>
            <Scale size={14} /><span>分类比价</span>
          </button>
          <button type="button" className={view === 'stores' ? 'active' : ''} onClick={() => setView('stores')}>
            <Store size={14} /><span>按店铺</span>
          </button>
          <button type="button" className={view === 'all' ? 'active' : ''} onClick={() => setView('all')}>
            <Boxes size={14} /><span>全部商品</span>
          </button>
        </div>
        <div className="market-price-scope" aria-label="价格层筛选">
          <button type="button" className={priceScope === 'bargain' ? 'active' : ''} onClick={() => setPriceScope('bargain')}>低价</button>
          <button type="button" className={priceScope === 'affordable' ? 'active' : ''} onClick={() => setPriceScope('affordable')}>合理价内</button>
          <button type="button" className={priceScope === 'all' ? 'active' : ''} onClick={() => setPriceScope('all')}>全部价格</button>
        </div>
      </div>

      <div className="market-filter-bar">
        <label className="market-search-field">
          <Search size={14} />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索商品或店铺" />
        </label>
        <label className="market-filter-field">
          <span>店铺</span>
          <select value={shopToken} onChange={(event) => setShopToken(event.target.value)}>
            <option value="all">全部店铺</option>
            {shops.filter((shop) => shop.enabled).map((shop) => (
              <option value={shop.token} key={shop.token}>{shop.name}</option>
            ))}
          </select>
        </label>
        <div className="market-price-filter" aria-label="到手价范围">
          <label>
            <span>最低价</span>
            <input min="0" step="1" inputMode="decimal" type="number" value={minimumPrice} onChange={(event) => setMinimumPrice(event.target.value)} />
          </label>
          <span>至</span>
          <label>
            <span>最高价</span>
            <input min="0" step="1" inputMode="decimal" type="number" value={maximumPrice} onChange={(event) => setMaximumPrice(event.target.value)} />
          </label>
        </div>
      </div>

      {category === 'gptplus' && (
        <div className="market-verification-filter" aria-label="GPT Plus 接码状态">
          <span>接码状态</span>
          {([
            ['all', '全部'],
            ['verified', '已接码'],
            ['unverified', '未接码'],
            ['unknown', '未知'],
          ] as Array<[VerificationFilter, string]>).map(([value, label]) => (
            <button
              type="button"
              className={verificationFilter === value ? 'active' : ''}
              onClick={() => setVerificationFilter(value)}
              key={value}
            >
              {label}
            </button>
          ))}
        </div>
      )}

      <div className="market-result-bar">
        <span className="market-result-meta">
          {visible.length} 个有货商品 · {productViewLabel(view)} · {priceScopeLabel(priceScope)}
        </span>
        <nav className="market-pagination" aria-label="商品分页">
          <span>第 {currentPage}/{totalPages} 页、当前 {pageFirst}-{pageLast}/共 {visible.length}</span>
          <div className="market-pagination-actions">
            <button
              className="icon-btn market-page-button"
              type="button"
              aria-label="上一页"
              data-tooltip="上一页"
              disabled={currentPage <= 1}
              onClick={() => setPage((current) => Math.max(1, current - 1))}
            >
              <ChevronLeft size={15} />
            </button>
            <button
              className="icon-btn market-page-button"
              type="button"
              aria-label="下一页"
              data-tooltip="下一页"
              disabled={currentPage >= totalPages}
              onClick={() => setPage((current) => Math.min(totalPages, current + 1))}
            >
              <ChevronRight size={15} />
            </button>
          </div>
        </nav>
      </div>
      {!groups.length ? (
        <MarketEmpty icon={<PackageSearch size={27} />} title="没有匹配的商品" detail="调整搜索、分类、店铺、价格范围或接码状态。" />
      ) : (
        <div className="market-product-groups">
          {groups.map((group) => (
            <section className={`market-product-group market-product-group-${view}`} key={group.key}>
              {view !== 'all' && (
                <header className="market-product-group-head">
                  <div>
                    <strong>{group.title}</strong>
                    <span>{productGroupDetail(view, group.items)}</span>
                  </div>
                  <div>
                    <small>最低到手价</small>
                    <strong>¥{formatPrice(group.items[0]?.totalPrice ?? 0)}</strong>
                  </div>
                </header>
              )}
              <div className="market-product-list">
                {group.items.map((product, index) => {
                  const priceTier = getProductPriceTier(product, priceProfiles)
                  const dedupe = shopCountByName.get(product.name.trim().toLocaleLowerCase())
                  const multiShop = dedupe && dedupe.shops > 1
                  return (
                    <button
                      className={`market-product-row${index === 0 ? ' cheapest' : ''}`}
                      type="button"
                      onClick={() => onOpen(product)}
                      key={product.id}
                    >
                      <span className="market-product-copy">
                        <strong>{product.name}</strong>
                        <small>
                          {product.shopName}
                          {multiShop ? ` · 共 ${dedupe.shops} 个报价` : ''}
                          {' · 库存 '}
                          {formatNumber(multiShop ? dedupe.stock : product.stockCount)}
                          {product.category === 'gptplus' ? ` · ${verificationLabel(product.verificationStatus)}` : ''}
                        </small>
                      </span>
                      {priceTier && (
                        <span className={`market-product-tier market-price-tier-${priceTier}`}>
                          {priceTierLabel(priceTier)}
                        </span>
                      )}
                      <span className="market-product-price">
                        <strong>¥{formatPrice(product.totalPrice)}</strong>
                        <small>{product.fee > 0 ? `含手续费 ¥${formatPrice(product.fee)}` : index === 0 ? '当前最低' : '到手价'}</small>
                      </span>
                      <ExternalLink size={14} />
                    </button>
                  )
                })}
              </div>
            </section>
          ))}
        </div>
      )}
    </section>
  )
}

function StoreSection({
  shops,
  busyShops,
  onAdd,
  onEdit,
  onToggle,
  onDelete,
  onOpen,
}: {
  shops: MarketShop[]
  busyShops: Set<string>
  onAdd: () => void
  onEdit: (shop: MarketShop) => void
  onToggle: (shop: MarketShop) => void
  onDelete: (shop: MarketShop) => void
  onOpen: (shop: MarketShop) => void
}) {
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<'all' | 'online' | 'abnormal' | 'disabled'>('all')
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const visible = shops.filter((shop) => {
    if (filter === 'online' && (!shop.enabled || !shop.ok)) return false
    if (filter === 'abnormal' && (!shop.enabled || shop.ok)) return false
    if (filter === 'disabled' && shop.enabled) return false
    return !normalizedQuery || [shop.name, shop.fallbackName, shop.token]
      .some((value) => value.toLocaleLowerCase().includes(normalizedQuery))
  })

  return (
    <section className="market-workspace market-stores" aria-label="店铺状态">
      <div className="market-filter-bar">
        <label className="market-search-field">
          <Search size={14} />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索店铺或 token" />
        </label>
        <label className="market-filter-field">
          <span>状态</span>
          <select value={filter} onChange={(event) => setFilter(event.target.value as typeof filter)}>
            <option value="all">全部状态</option>
            <option value="online">在线</option>
            <option value="abnormal">异常</option>
            <option value="disabled">已停用</option>
          </select>
        </label>
        <button className="btn btn-primary market-add-shop" type="button" onClick={onAdd}>
          <Plus size={15} />添加店铺
        </button>
      </div>

      {!visible.length ? (
        <MarketEmpty icon={<Store size={27} />} title="没有匹配的店铺" detail="调整筛选条件或添加新的链动小铺。" />
      ) : (
        <div className="market-store-grid">
          {visible.map((shop) => {
            const busy = busyShops.has(shop.token)
            const state = !shop.enabled ? 'disabled' : shop.ok ? 'online' : 'abnormal'
            return (
              <article className={`market-store-card market-store-${state}`} key={shop.token}>
                <header className="market-store-head">
                  <div className="market-store-title">
                    <span className="market-store-state" aria-hidden="true" />
                    <div>
                      <strong>{shop.name}</strong>
                      <small>{shop.token}</small>
                    </div>
                  </div>
                  <span className="market-store-status">{storeStatusLabel(shop)}</span>
                </header>
                <div className="market-store-metrics">
                  <span><small>商品</small><strong>{formatNumber(shop.productCount)}</strong></span>
                  <span><small>总库存</small><strong>{formatNumber(shop.totalStock)}</strong></span>
                  <span><small>连续失败</small><strong>{formatNumber(shop.failureCount)}</strong></span>
                  <span><small>买家费率</small><strong>{shop.feePayer === 1 ? `${formatPrice(shop.feeRate)}%` : '商家承担'}</strong></span>
                </div>
                <div className="market-store-detail">
                  <span>最后成功：{shop.lastSuccessAt ? formatDateTime(shop.lastSuccessAt) : '暂无'}</span>
                  {shop.blockedUntil && <span>恢复探测：{formatDateTime(shop.blockedUntil)}</span>}
                  {shop.error && <span className="market-store-error">{shop.error}</span>}
                </div>
                <footer className="market-store-actions">
                  <button className="btn market-store-toggle" type="button" role="switch" aria-checked={shop.enabled} disabled={busy} onClick={() => onToggle(shop)}>
                    {shop.enabled ? '暂停监控' : '启用监控'}
                  </button>
                  <button className="icon-btn market-icon-action" type="button" data-tooltip="打开店铺" aria-label={`打开 ${shop.name}`} onClick={() => onOpen(shop)}>
                    <ExternalLink size={15} />
                  </button>
                  <button className="icon-btn market-icon-action" type="button" data-tooltip="编辑店铺" aria-label={`编辑 ${shop.name}`} disabled={busy} onClick={() => onEdit(shop)}>
                    <Pencil size={15} />
                  </button>
                  <button className="icon-btn market-icon-action market-icon-danger" type="button" data-tooltip="删除店铺" aria-label={`删除 ${shop.name}`} disabled={busy} onClick={() => onDelete(shop)}>
                    <Trash2 size={15} />
                  </button>
                </footer>
              </article>
            )
          })}
        </div>
      )}
    </section>
  )
}

function AnalyticsSection({
  data,
  loading,
  error,
  range,
  onRangeChange,
  onRetry,
}: {
  data: MarketAnalyticsSnapshot | null
  loading: boolean
  error: string
  range: MarketRange
  onRangeChange: (range: MarketRange) => void
  onRetry: () => void
}) {
  const first = data?.points[0]
  const latest = data?.points.at(-1)
  const stockChange = latest && first ? latest.totalStock - first.totalStock : 0
  const eventCounts = useMemo(() => ({
    surge: data?.events.filter((event) => event.kind === 'market.stock_surge').length ?? 0,
    price: data?.events.filter((event) => event.kind.startsWith('product.price_')).length ?? 0,
    unavailable: data?.events.filter((event) => event.kind === 'product.unavailable').length ?? 0,
  }), [data])

  return (
    <section className="market-workspace market-analytics" aria-label="行情分析">
      <div className="market-analytics-toolbar">
        <div className="market-range-tabs" aria-label="分析时间范围">
          {(['24h', '7d', '30d'] as const).map((value) => (
            <button className={range === value ? 'active' : ''} type="button" onClick={() => onRangeChange(value)} key={value}>
              {value === '24h' ? '24 小时' : value === '7d' ? '7 天' : '30 天'}
            </button>
          ))}
        </div>
        <span>{data ? `${formatNumber(data.totalSamples)} 个原始采样` : '等待分析数据'}</span>
        <button className="icon-btn market-icon-action" type="button" aria-label="刷新行情分析" data-tooltip="刷新分析" disabled={loading} onClick={onRetry}>
          <RefreshCw className={loading ? 'spin' : undefined} size={15} />
        </button>
      </div>

      {error && <div className="market-feedback market-feedback-error"><AlertCircle size={15} />{error}</div>}
      {loading && !data ? (
        <div className="market-loading"><RefreshCw className="spin" size={21} />正在汇总行情</div>
      ) : !data?.points.length ? (
        <MarketEmpty icon={<BarChart3 size={27} />} title="还没有行情样本" detail="完成一次商品采集后会开始记录价格与库存趋势。" />
      ) : (
        <>
          <div className="market-analytics-summary">
            <MarketSummary label="当前总库存" value={formatNumber(latest?.totalStock ?? 0)} tone={stockChange < 0 ? 'warning' : 'healthy'} />
            <MarketSummary label="当前商品" value={formatNumber(latest?.productCount ?? 0)} />
            <MarketSummary label="集中补库" value={formatNumber(eventCounts.surge)} tone={eventCounts.surge ? 'warning' : 'neutral'} />
            <MarketSummary label="价格信号" value={formatNumber(eventCounts.price)} tone={eventCounts.price ? 'warning' : 'neutral'} />
          </div>
          <div className="market-chart-panel">
            <header><strong>库存走势</strong><span>{rangeLabel(range)}</span></header>
            <StockTrendChart points={data.points} />
          </div>
          <div className="market-category-grid">
            {(latest?.categories ?? []).map((metric) => (
              <article className="market-category-card" key={metric.key}>
                <header><strong>{metric.label || categoryLabel(metric.key)}</strong><span>{formatNumber(metric.productCount)} 款</span></header>
                <div><small>总库存</small><strong>{formatNumber(metric.totalStock)}</strong></div>
                <div><small>加权均价</small><strong>¥{formatPrice(metric.weightedAveragePrice)}</strong></div>
                <div><small>当前最低</small><strong>¥{formatPrice(metric.minimumPrice)}</strong></div>
              </article>
            ))}
          </div>
          <div className="market-signal-list">
            <header><strong>最近市场信号</strong><span>{data.events.length} 条</span></header>
            {data.events.slice(0, 12).map((event) => <MarketEventRow event={event} key={event.eventId} />)}
            {!data.events.length && <span className="market-muted">所选区间没有异常信号。</span>}
          </div>
        </>
      )}
    </section>
  )
}

function AlertsSection({
  alerts,
  settings,
  settingsDirty,
  saving,
  notificationPermission,
  permissionRequesting,
  onSettingsChange,
  onSave,
  onRequestPermission,
  onMarkRead,
  onOpen,
}: {
  alerts: MarketEvent[]
  settings: MarketAlertSettings
  settingsDirty: boolean
  saving: boolean
  notificationPermission: MarketNotificationPermission
  permissionRequesting: boolean
  onSettingsChange: (settings: MarketAlertSettings) => void
  onSave: () => void
  onRequestPermission: () => void
  onMarkRead: (ids?: string[]) => void
  onOpen: (event: MarketEvent) => void
}) {
  const [filter, setFilter] = useState<'all' | 'unread'>('all')
  const [showSoldOut, setShowSoldOut] = useState(true)
  const [showRestock, setShowRestock] = useState(true)
  const [page, setPage] = useState(1)
  const unread = alerts.filter((event) => !event.readAt)
  const visible = useMemo(() => {
    let list = filter === 'unread' ? alerts.filter((event) => !event.readAt) : alerts
    if (!showSoldOut) list = list.filter((event) => event.kind !== 'product.unavailable')
    if (!showRestock) list = list.filter((event) => event.kind !== 'product.available' && event.kind !== 'market.stock_surge')
    return list
  }, [alerts, filter, showSoldOut, showRestock])
  const totalPages = Math.max(1, Math.ceil(visible.length / ALERT_PAGE_SIZE))
  const currentPage = Math.min(page, totalPages)
  const pageAlerts = useMemo(
    () => visible.slice((currentPage - 1) * ALERT_PAGE_SIZE, currentPage * ALERT_PAGE_SIZE),
    [currentPage, visible],
  )
  useEffect(() => { setPage(1) }, [filter, showSoldOut, showRestock])
  useEffect(() => { setPage((current) => Math.min(Math.max(1, current), totalPages)) }, [totalPages])
  const update = <K extends keyof MarketAlertSettings>(key: K, value: MarketAlertSettings[K]) => {
    onSettingsChange({ ...settings, [key]: value })
  }

  return (
    <section className="market-workspace market-alerts" aria-label="市场提醒">
      <div className="market-alert-layout">
        <section className="market-alert-settings">
          <header className="market-panel-head">
            <div><BellRing size={17} /><strong>提醒规则</strong></div>
            <button className="btn btn-primary market-save-settings" type="button" disabled={saving || !settingsDirty} onClick={onSave}>
              {saving ? '保存中' : '保存规则'}
            </button>
          </header>
          <SettingToggle label="桌面通知总开关" detail="事件始终入库；关闭只停桌面通知，K12/GPT Plus 到货无单独开关" checked={settings.enabled} onChange={(value) => update('enabled', value)} />
          <SettingToggle label="系统通知" detail="通过 Windows 通知中心发送符合规则的事件" checked={settings.nativeEnabled} disabled={!settings.enabled} onChange={(value) => update('nativeEnabled', value)} />
          <div className={`market-notification-permission market-permission-${notificationPermission}`} role="status">
            {notificationPermission === 'granted' ? <CheckCircle2 size={16} /> : <AlertCircle size={16} />}
            <span className="market-permission-copy">
              <strong>{notificationPermissionTitle(notificationPermission)}</strong>
              <small>{notificationPermissionDetail(notificationPermission)}</small>
            </span>
            {(notificationPermission === 'prompt' || notificationPermission === 'denied') && (
              <button className="btn" type="button" disabled={permissionRequesting} onClick={onRequestPermission}>
                <BellRing size={14} />
                {permissionRequesting ? '请求中' : notificationPermission === 'denied' ? '重新授权' : '授权'}
              </button>
            )}
          </div>
          <div className="market-setting-divider" />
          <SettingToggle label="BUG TEAM 到货" detail="首次出现有货商品时发送桌面通知" checked={settings.bugTeamAvailable} disabled={!settings.enabled} onChange={(value) => update('bugTeamAvailable', value)} />
          <SettingToggle label="商品售罄" detail="连续两次采集确认缺货后发送桌面通知" checked={settings.productUnavailable} disabled={!settings.enabled} onChange={(value) => update('productUnavailable', value)} />
          <SettingToggle label="店铺健康" detail="连续失败或恢复时发送桌面通知" checked={settings.storeHealth} disabled={!settings.enabled} onChange={(value) => update('storeHealth', value)} />
          <SettingToggle label="集中补库" detail="集中补库或疑似扫货/库存转移时发送桌面通知" checked={settings.stockSurge} disabled={!settings.enabled} onChange={(value) => update('stockSurge', value)} />
          <SettingToggle label="价格异常" detail="发现明显低价或高价商品时发送桌面通知" checked={settings.priceOutlier} disabled={!settings.enabled} onChange={(value) => update('priceOutlier', value)} />
          <div className="market-setting-divider" />
          <SettingToggle label="静默时段" detail="静默期间仍记录事件，但不发送系统通知" checked={settings.quietHoursEnabled} disabled={!settings.enabled} onChange={(value) => update('quietHoursEnabled', value)} />
          <div className="market-quiet-hours">
            <label><span>开始</span><input type="time" value={settings.quietHoursStart} disabled={!settings.quietHoursEnabled || !settings.enabled} onChange={(event) => update('quietHoursStart', event.target.value)} /></label>
            <span>至</span>
            <label><span>结束</span><input type="time" value={settings.quietHoursEnd} disabled={!settings.quietHoursEnabled || !settings.enabled} onChange={(event) => update('quietHoursEnd', event.target.value)} /></label>
          </div>
        </section>

        <section className="market-alert-history">
          <header className="market-panel-head">
            <div><Bell size={17} /><strong>提醒历史</strong><span>{unread.length} 条未读</span></div>
            <button className="btn market-mark-all" type="button" disabled={!unread.length} onClick={() => onMarkRead()}>
              全部已读
            </button>
          </header>
          <div className="market-alert-filter" aria-label="提醒筛选">
            <button type="button" className={filter === 'all' ? 'active' : ''} onClick={() => setFilter('all')}>全部 {alerts.length}</button>
            <button type="button" className={filter === 'unread' ? 'active' : ''} onClick={() => setFilter('unread')}>未读 {unread.length}</button>
          </div>
          <div className="market-alert-kind-toggles" aria-label="类型筛选">
            <label className={`market-kind-chip${showSoldOut ? ' active' : ''}`}>
              <input type="checkbox" checked={showSoldOut} onChange={(event) => setShowSoldOut(event.target.checked)} />
              售罄
            </label>
            <label className={`market-kind-chip${showRestock ? ' active' : ''}`}>
              <input type="checkbox" checked={showRestock} onChange={(event) => setShowRestock(event.target.checked)} />
              补货
            </label>
          </div>
          {!visible.length ? (
            <MarketEmpty icon={<Bell size={25} />} title={filter === 'unread' ? '没有未读提醒' : '暂无提醒'} detail="新到货、店铺异常和市场信号会出现在这里。" />
          ) : (
            <>
            <div className="market-alert-list">
              {pageAlerts.map((event) => {
                const canOpen = Boolean(payloadText(event.payload, 'url'))
                return (
                  <article className={`market-alert-row market-severity-${event.severity}${event.readAt ? ' read' : ' unread'}`} key={event.eventId}>
                    <span className="market-alert-indicator" aria-hidden="true" />
                    <div className="market-alert-copy">
                      <header><strong>{event.title}</strong><time>{formatDateTime(event.occurredAt)}</time></header>
                      <p>{event.body}</p>
                      <small>{eventKindLabel(event.kind)}{event.notifiedAt ? ' · 已发送系统通知' : ''}</small>
                    </div>
                    <div className="market-alert-actions">
                      {canOpen && <button type="button" onClick={() => onOpen(event)}>查看</button>}
                      {!event.readAt && <button type="button" onClick={() => onMarkRead([event.eventId])}>已读</button>}
                    </div>
                  </article>
                )
              })}
            </div>
            {totalPages > 1 && (
              <nav className="market-pagination market-alert-pagination" aria-label="提醒分页">
                <span>第 {currentPage}/{totalPages} 页 · 共 {visible.length} 条</span>
                <div className="market-pagination-actions">
                  <button className="icon-btn market-page-button" type="button" aria-label="上一页" disabled={currentPage <= 1} onClick={() => setPage((current) => Math.max(1, current - 1))}>
                    <ChevronLeft size={15} />
                  </button>
                  <button className="icon-btn market-page-button" type="button" aria-label="下一页" disabled={currentPage >= totalPages} onClick={() => setPage((current) => Math.min(totalPages, current + 1))}>
                    <ChevronRight size={15} />
                  </button>
                </div>
              </nav>
            )}
            </>
          )}
        </section>
      </div>
    </section>
  )
}

function SettingToggle({
  label,
  detail,
  checked,
  disabled = false,
  onChange,
}: {
  label: string
  detail: string
  checked: boolean
  disabled?: boolean
  onChange: (checked: boolean) => void
}) {
  return (
    <label className={`market-setting-toggle${disabled ? ' disabled' : ''}`}>
      <span><strong>{label}</strong><small>{detail}</small></span>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
      <span className="market-setting-switch" aria-hidden="true"><span /></span>
    </label>
  )
}

function StockTrendChart({ points }: { points: MarketAnalyticsPoint[] }) {
  const width = 720
  const height = 180
  const paddingX = 18
  const paddingY = 18
  const values = points.map((point) => point.totalStock)
  const minimum = Math.min(...values)
  const maximum = Math.max(...values)
  const spread = Math.max(1, maximum - minimum)
  const coordinates = points.map((point, index) => ({
    x: paddingX + index / Math.max(1, points.length - 1) * (width - paddingX * 2),
    y: height - paddingY - (point.totalStock - minimum) / spread * (height - paddingY * 2),
  }))
  const line = coordinates.map((point) => `${point.x},${point.y}`).join(' ')
  const area = `${paddingX},${height - paddingY} ${line} ${width - paddingX},${height - paddingY}`

  return (
    <div className="market-stock-chart">
      <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`库存从 ${values[0]} 变化到 ${values.at(-1)}`}>
        <line className="market-chart-grid" x1={paddingX} x2={width - paddingX} y1={paddingY} y2={paddingY} />
        <line className="market-chart-grid" x1={paddingX} x2={width - paddingX} y1={height / 2} y2={height / 2} />
        <line className="market-chart-grid" x1={paddingX} x2={width - paddingX} y1={height - paddingY} y2={height - paddingY} />
        <polygon className="market-chart-area" points={area} />
        <polyline className="market-chart-line" points={line} />
      </svg>
      <div className="market-chart-scale"><span>{formatNumber(maximum)}</span><span>{formatNumber(minimum)}</span></div>
      <div className="market-chart-times"><span>{formatShortDate(points[0]?.capturedAt)}</span><span>{formatShortDate(points.at(-1)?.capturedAt)}</span></div>
    </div>
  )
}

function MarketEventRow({ event }: { event: MarketEvent }) {
  return (
    <div className={`market-signal-row market-severity-${event.severity}`}>
      <span className="market-signal-icon" aria-hidden="true" />
      <div><strong>{event.title}</strong><span>{event.body}</span></div>
      <time>{formatDateTime(event.occurredAt)}</time>
    </div>
  )
}

function MarketEmpty({ icon, title, detail }: { icon: ReactNode; title: string; detail: string }) {
  return (
    <div className="market-empty">
      {icon}
      <strong>{title}</strong>
      <span>{detail}</span>
    </div>
  )
}

function ShopEditorDialog({
  editor,
  busy,
  onChange,
  onClose,
  onSave,
}: {
  editor: ShopEditorState | null
  busy: boolean
  onChange: (editor: ShopEditorState | null) => void
  onClose: () => void
  onSave: () => void
}) {
  const update = <K extends keyof MarketShopInput>(key: K, value: MarketShopInput[K]) => {
    if (editor) onChange({ ...editor, input: { ...editor.input, [key]: value } })
  }
  return (
    <Dialog
      open={Boolean(editor)}
      title={editor?.originalToken ? '编辑监控店铺' : '添加监控店铺'}
      onClose={onClose}
      preventClose={busy}
      small
      footer={(
        <>
          <button className="btn market-dialog-cancel" type="button" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-primary market-dialog-save" type="button" onClick={onSave} disabled={busy}>
            {busy ? '保存中' : '保存店铺'}
          </button>
        </>
      )}
    >
      {editor && (
        <div className="market-shop-form">
          <label>
            <span>平台</span>
            <select value={editor.input.platform} disabled>
              <option value="liandx">链动小铺</option>
            </select>
          </label>
          <label>
            <span>店铺名称</span>
            <input value={editor.input.fallbackName} onChange={(event) => update('fallbackName', event.target.value)} placeholder="用于加载前和异常时显示" autoFocus />
          </label>
          <label>
            <span>店铺 token</span>
            <input value={editor.input.token} onChange={(event) => update('token', event.target.value)} placeholder="例如 echo_dream" disabled={Boolean(editor.originalToken)} />
            <small className="market-shop-hint">即店铺链接最后一段，如 pay.ldxp.cn/shop/<b>echo_dream</b></small>
          </label>
          <label className="market-shop-enabled">
            <input type="checkbox" checked={editor.input.enabled} onChange={(event) => update('enabled', event.target.checked)} />
            <span>保存后立即启用监控</span>
          </label>
        </div>
      )}
    </Dialog>
  )
}

function DeleteShopDialog({
  shop,
  busy,
  onClose,
  onConfirm,
}: {
  shop: MarketShop | null
  busy: boolean
  onClose: () => void
  onConfirm: () => void
}) {
  return (
    <Dialog
      open={Boolean(shop)}
      title="删除监控店铺"
      onClose={onClose}
      preventClose={busy}
      small
      footer={(
        <>
          <button className="btn market-dialog-cancel" type="button" onClick={onClose} disabled={busy}>取消</button>
          <button className="btn btn-danger market-dialog-delete" type="button" onClick={onConfirm} disabled={busy}>
            {busy ? '删除中' : '删除店铺'}
          </button>
        </>
      )}
    >
      <div className="market-delete-copy">
        <AlertCircle size={20} />
        <p>确认删除「{shop?.name}」？该店铺的当前商品缓存也会被移除，既有提醒历史会保留。</p>
      </div>
    </Dialog>
  )
}

function protectionLabel(snapshot: MarketSnapshot | null) {
  switch (snapshot?.protection.mode) {
    case 'backoff': return '退避中'
    case 'circuit-open': return '熔断中'
    default: return snapshot?.status === 'error' ? '采集异常' : '正常'
  }
}

function buildProductPriceProfiles(products: MarketProduct[]) {
  const grouped = new Map<string, number[]>()
  for (const product of products) {
    if (!product.category || product.totalPrice <= 0 || product.stockCount <= 0) continue
    const prices = grouped.get(product.category)
    if (prices) prices.push(product.totalPrice)
    else grouped.set(product.category, [product.totalPrice])
  }

  const profiles = new Map<string, ProductPriceProfile>()
  for (const [category, prices] of grouped) {
    const median = floorQuantile(prices, 0.5)
    const mad = floorQuantile(prices.map((price) => Math.abs(price - median)), 0.5)
    const affordableCeiling = prices.length < 4
      ? Number.POSITIVE_INFINITY
      : roundMoney(Math.max(median * 1.15, median + 3 * 1.4826 * mad))
    const affordablePrices = prices.filter((price) => price <= affordableCeiling)
    profiles.set(category, {
      median: roundMoney(median),
      bargain: roundMoney(floorQuantile(affordablePrices, 0.25)),
      affordableCeiling,
    })
  }
  return profiles
}

function floorQuantile(values: number[], ratio: number) {
  if (!values.length) return 0
  const sorted = [...values].sort((left, right) => left - right)
  return sorted[Math.floor((sorted.length - 1) * ratio)]
}

function roundMoney(value: number) {
  return Math.round(value * 100) / 100
}

function getProductPriceTier(
  product: MarketProduct,
  profiles: Map<string, ProductPriceProfile>,
): ProductPriceTier | null {
  if (!product.category || product.totalPrice <= 0) return null
  const profile = profiles.get(product.category)
  if (!profile) return null
  if (product.totalPrice <= profile.bargain) return 'bargain'
  if (product.totalPrice <= profile.affordableCeiling) return 'affordable'
  return 'high'
}

function matchesPriceScope(
  product: MarketProduct,
  scope: ProductPriceScope,
  profiles: Map<string, ProductPriceProfile>,
) {
  if (scope === 'all') return true
  const tier = getProductPriceTier(product, profiles)
  if (scope === 'bargain') return tier === 'bargain'
  return tier === 'bargain' || tier === 'affordable'
}

function productViewLabel(view: ProductView) {
  if (view === 'stores') return '按店铺分组'
  if (view === 'all') return '全部平铺'
  return '同分类跨店比价'
}

function priceScopeLabel(scope: ProductPriceScope) {
  if (scope === 'bargain') return '只看低价'
  if (scope === 'affordable') return '合理价内'
  return '全部价格'
}

function priceTierLabel(tier: ProductPriceTier) {
  if (tier === 'bargain') return '低价'
  if (tier === 'affordable') return '合理'
  return '偏高'
}

function productGroupDetail(view: ProductView, products: MarketProduct[]) {
  if (view === 'stores') {
    const categories = new Set(products.map((product) => product.category || 'other')).size
    const stock = products.reduce((total, product) => total + product.stockCount, 0)
    return `${categories} 个分类 · 库存 ${formatNumber(stock)}`
  }
  return `${new Set(products.map((product) => product.shopToken)).size} 家店铺 · ${products.length} 个报价`
}

function notificationPermissionTitle(permission: MarketNotificationPermission) {
  if (permission === 'granted') return '系统通知已授权'
  if (permission === 'denied') return '系统通知已拒绝'
  if (permission === 'unavailable') return '当前环境不可用'
  if (permission === 'checking') return '正在检查通知权限'
  return '系统通知尚未授权'
}

function notificationPermissionDetail(permission: MarketNotificationPermission) {
  if (permission === 'granted') return '保存规则后，符合条件的事件会发送桌面通知。'
  if (permission === 'denied') return '请重新授权，或在系统设置中允许 Aether 发送通知。'
  if (permission === 'unavailable') return '应用内提醒仍可正常记录和查看。'
  if (permission === 'checking') return '正在读取系统通知状态。'
  return '授权操作只会在点击按钮后发起。'
}

function categoryOrder(value: string) {
  return ['k12', 'gptplus', 'bugteam', 'other'].indexOf(value) < 0
    ? 99
    : ['k12', 'gptplus', 'bugteam', 'other'].indexOf(value)
}

function categoryLabel(value: string) {
  return categoryLabels[value] || value || categoryLabels.other
}

function verificationLabel(value: string) {
  if (value === 'verified') return '已接码'
  if (value === 'unverified') return '未接码'
  return '接码未知'
}

function storeStatusLabel(shop: MarketShop) {
  if (!shop.enabled) return '已停用'
  if (shop.ok) return '在线'
  return shop.blockedUntil ? '恢复等待' : '异常'
}

function eventKindLabel(kind: string) {
  switch (kind) {
    case 'product.available': return '商品到货'
    case 'product.unavailable': return '商品售罄'
    case 'store.degraded': return '店铺异常'
    case 'store.recovered': return '店铺恢复'
    case 'market.stock_surge': return '集中补库'
    case 'market.suspected_hoarding': return '疑似扫货/库存转移'
    case 'product.price_low': return '低价信号'
    case 'product.price_high': return '高价信号'
    default: return '市场事件'
  }
}

function payloadText(payload: Record<string, unknown>, key: string) {
  const value = payload[key]
  return typeof value === 'string' ? value : ''
}

function rangeLabel(range: MarketRange) {
  if (range === '7d') return '过去 7 天'
  if (range === '30d') return '过去 30 天'
  return '过去 24 小时'
}

function formatPrice(value: number) {
  if (!Number.isFinite(value)) return '-'
  return new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 2 }).format(value)
}

function formatNumber(value: number) {
  return new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 0 }).format(value)
}
