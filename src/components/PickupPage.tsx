import { useCallback, useEffect, useRef, useState } from 'react'
import { ExternalLink, PackagePlus, RefreshCw, Settings2 } from 'lucide-react'
import {
  createPickupOrder,
  getPickupOverview,
  getPickupSettings,
  listPickupOrders,
  refreshPickupOrder,
  retryPickupOrderImport,
} from '../lib/commands'
import type { PickupImportResult, PickupOrderRecord, PickupOverview, PickupSettings } from '../types'

interface PickupPageProps {
  onOpenSettings: () => void
  onAccountsImported?: (result: PickupImportResult) => void
}

const PENDING_ORDER_KEY = 'aether:pickup-pending-order:v1'

export function PickupPage({ onOpenSettings, onAccountsImported }: PickupPageProps) {
  const [settings, setSettings] = useState<PickupSettings | null>(null)
  const [overview, setOverview] = useState<PickupOverview | null>(null)
  const [orders, setOrders] = useState<PickupOrderRecord[]>([])
  const [quantity, setQuantity] = useState(1)
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState('')
  const importedOrders = useRef(new Set<string>())

  const notifyImported = useCallback((order: PickupOrderRecord) => {
    if (!order.order_id || !order.import_result || importedOrders.current.has(order.order_id)) return
    importedOrders.current.add(order.order_id)
    onAccountsImported?.(order.import_result)
  }, [onAccountsImported])

  const mergeOrder = useCallback((next: PickupOrderRecord, notify = true) => {
    setOrders((current) => {
      const without = current.filter((item) => item.order_id !== next.order_id
        && item.idempotency_key !== next.idempotency_key)
      return [next, ...without].sort((left, right) => right.created_at.localeCompare(left.created_at))
    })
    if (notify) notifyImported(next)
  }, [notifyImported])

  const loadOverview = useCallback(async (nextQuantity = quantity) => {
    if (!settings?.customer_token) return
    setRefreshing(true)
    try {
      setOverview(await getPickupOverview(nextQuantity))
      setError('')
    } catch (reason) {
      setError(errorText(reason))
    } finally {
      setRefreshing(false)
    }
  }, [quantity, settings?.customer_token])

  const loadPage = useCallback(async () => {
    setLoading(true)
    try {
      const [nextSettings, nextOrders] = await Promise.all([getPickupSettings(), listPickupOrders()])
      setSettings(nextSettings)
      setOrders(nextOrders)
      importedOrders.current = new Set(
        nextOrders.filter((order) => order.import_result).map((order) => order.order_id),
      )
      if (nextSettings.customer_token) {
        setOverview(await getPickupOverview(quantity))
        setError('')
      }
    } catch (reason) {
      setError(errorText(reason))
    } finally {
      setLoading(false)
    }
  }, [quantity])

  useEffect(() => { void loadPage() }, [loadPage])

  const refreshOrder = useCallback(async (orderId: string) => {
    try {
      const next = await refreshPickupOrder(orderId)
      mergeOrder(next)
      if (next.import_result || next.import_error) void loadOverview()
      setError('')
    } catch (reason) {
      setError(errorText(reason))
    }
  }, [loadOverview, mergeOrder])

  useEffect(() => {
    const active = orders.find((order) => order.order_id && !isFinished(order.state))
    if (!active) return undefined
    const timer = window.setInterval(() => { void refreshOrder(active.order_id) }, 3000)
    return () => window.clearInterval(timer)
  }, [orders, refreshOrder])

  const submitOrder = async () => {
    if (submitting) return
    if (!settings?.customer_token) {
      onOpenSettings()
      return
    }
    const safeQuantity = Math.max(1, Math.min(1000, Math.trunc(quantity || 1)))
    const estimated = fenValue(overview?.inventory?.hold_total_fen)
    const confirmation = estimated > 0
      ? `将为 ${safeQuantity} 个 Team 账号创建订单，预计锁款 ¥${formatMoney(estimated)}。确定继续吗？`
      : `将为 ${safeQuantity} 个 Team 账号创建订单并锁定余额，确定继续吗？`
    if (!window.confirm(confirmation)) return

    const idempotencyKey = pendingIdempotencyKey(safeQuantity)
    setSubmitting(true)
    setError('')
    try {
      const next = await createPickupOrder(safeQuantity, idempotencyKey)
      clearPendingIdempotency()
      mergeOrder(next)
      await loadOverview(safeQuantity)
    } catch (reason) {
      setError(`${errorText(reason)}；如果服务端已受理，请稍后刷新订单状态`)
    } finally {
      setSubmitting(false)
    }
  }

  const retryImport = async (orderId: string) => {
    try {
      const next = await retryPickupOrderImport(orderId)
      mergeOrder(next)
      setError('')
    } catch (reason) {
      setError(errorText(reason))
    }
  }

  const balance = overview?.balance ?? {}
  const inventory = overview?.inventory ?? {}
  const available = fenValue(balance.available_fen)
  const stock = numberValue(inventory.available)
  const holdTotal = fenValue(inventory.hold_total_fen)
  const unitPrice = fenValue(inventory.estimated_unit_price_fen ?? inventory.base_unit_price_fen)

  return (
    <main className="pickup-page">
      <header className="pickup-header">
        <div>
          <div className="pickup-kicker">SUPPLY API</div>
          <h1><PackagePlus size={21} aria-hidden="true" /> Team 取号</h1>
          <p>从 sub2api 领取 Team 账号，完成后自动导入 Aether。</p>
        </div>
        <button className="btn" type="button" onClick={() => { void loadPage() }} disabled={loading || refreshing}>
          <RefreshCw size={14} className={loading || refreshing ? 'spin' : ''} /> 刷新
        </button>
      </header>

      {!settings?.customer_token && !loading && (
        <section className="pickup-notice">
          <div>
            <strong>还没有配置 Customer Token</strong>
            <span>Token 只保存在本机设置中，由 Aether 后端直接调用取号接口。</span>
          </div>
          <button className="btn btn-primary" type="button" onClick={onOpenSettings}>
            <Settings2 size={14} /> 打开设置
          </button>
        </section>
      )}

      {error && <div className="pickup-error" role="alert">{error}</div>}

      <section className="pickup-metrics" aria-label="余额和库存">
        <Metric label="可用余额" value={settings?.customer_token ? `¥${formatMoney(available)}` : '—'} />
        <Metric label="当前库存" value={settings?.customer_token ? `${formatNumber(stock)} 个` : '—'} />
        <Metric label="预计单价" value={unitPrice > 0 ? `¥${formatMoney(unitPrice)}` : '—'} />
        <Metric label="本次数量锁款" value={holdTotal > 0 ? `¥${formatMoney(holdTotal)}` : '—'} />
      </section>

      <section className="pickup-grid">
        <div className="pickup-card pickup-order-form">
          <div className="pickup-card-head"><div><h2>创建取号订单</h2><span>商品：Team 1 小时（team_1h）</span></div></div>
          <div className="pickup-form-row">
            <label htmlFor="pickup-quantity">数量</label>
            <input
              id="pickup-quantity"
              type="number"
              min={1}
              max={1000}
              value={quantity}
              onChange={(event) => setQuantity(Math.max(1, Math.min(1000, Number(event.target.value) || 1)))}
              onBlur={() => { void loadOverview() }}
              disabled={!settings?.customer_token || submitting}
            />
            <button className="btn btn-primary" type="button" onClick={() => { void submitOrder() }} disabled={submitting || loading}>
              <PackagePlus size={14} /> {submitting ? '提交中…' : '创建订单'}
            </button>
          </div>
          <p className="pickup-hint">订单会按 FIFO 自动履约；提交超时后再次操作会复用同一幂等键，避免重复扣款。</p>
        </div>

        <div className="pickup-card">
          <div className="pickup-card-head"><div><h2>订单状态</h2><span>完成后自动下载 Sub2 JSON 并导入</span></div></div>
          <div className="pickup-orders">
            {orders.length === 0 && <div className="pickup-empty">暂无订单记录</div>}
            {orders.slice(0, 8).map((order) => (
              <OrderRow key={order.idempotency_key || order.order_id} order={order} onRetryImport={() => { void retryImport(order.order_id) }} />
            ))}
          </div>
        </div>
      </section>

      <footer className="pickup-footer">
        <span>接口：bugteam.team · Customer Token 不会写入请求日志</span>
        <a href="https://bugteam.team" target="_blank" rel="noreferrer"><ExternalLink size={12} /> 服务说明</a>
      </footer>
    </main>
  )
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="pickup-metric"><span>{label}</span><strong>{value}</strong></div>
}

function OrderRow({ order, onRetryImport }: { order: PickupOrderRecord; onRetryImport: () => void }) {
  const finished = isFinished(order.state)
  const imported = order.import_result && order.import_result.failed === 0
  return (
    <div className="pickup-order-row">
      <div className={`pickup-order-status ${finished ? 'done' : 'pending'}`} />
      <div className="pickup-order-main">
        <strong>{order.product} × {order.quantity}</strong>
        <span>{order.order_id || '等待服务端确认'} · {formatDate(order.updated_at || order.created_at)}</span>
      </div>
      <div className="pickup-order-result">
        <b>{stateLabel(order.state)}</b>
        {imported && <span className="pickup-import-ok">已入库 {(order.import_result?.created ?? 0) + (order.import_result?.updated ?? 0)} 个</span>}
        {order.import_result && order.import_result.failed > 0 && <button type="button" onClick={onRetryImport}>重试导入</button>}
        {order.import_error && <span className="pickup-import-error" title={order.import_error}>下载失败</span>}
        {order.last_error && <span className="pickup-import-error" title={order.last_error}>需重试</span>}
      </div>
    </div>
  )
}

function pendingIdempotencyKey(quantity: number) {
  try {
    const raw = window.localStorage.getItem(PENDING_ORDER_KEY)
    if (raw) {
      const saved = JSON.parse(raw) as { quantity?: number; key?: string; created_at?: number }
      if (saved.quantity === quantity && saved.key && Date.now() - (saved.created_at ?? 0) < 86_400_000) return saved.key
    }
  } catch { /* ignored */ }
  const key = typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
  try { window.localStorage.setItem(PENDING_ORDER_KEY, JSON.stringify({ quantity, key, created_at: Date.now() })) } catch { /* ignored */ }
  return key
}

function clearPendingIdempotency() {
  try { window.localStorage.removeItem(PENDING_ORDER_KEY) } catch { /* ignored */ }
}

function isFinished(state: string) {
  return ['completed', 'complete', 'fulfilled', 'delivered', 'success', 'cancelled', 'failed', 'expired'].includes(state.toLowerCase())
}

function stateLabel(state: string) {
  const labels: Record<string, string> = {
    submitting: '提交中',
    submit_unknown: '待确认',
    created: '已下单',
    pending: '等待补货',
    reserving: '锁款中',
    completed: '已完成',
    complete: '已完成',
    fulfilled: '已完成',
    delivered: '已交付',
    success: '已完成',
    cancelled: '已取消',
    failed: '失败',
    expired: '已过期',
  }
  return labels[state.toLowerCase()] ?? state
}

function numberValue(value: unknown) {
  const parsed = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(parsed) ? parsed : 0
}

function fenValue(value: unknown) {
  return numberValue(value) / 100
}

function formatMoney(value: number) {
  return value.toFixed(2)
}

function formatNumber(value: number) {
  return new Intl.NumberFormat('zh-CN').format(value)
}

function formatDate(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '刚刚' : date.toLocaleString('zh-CN', { hour12: false })
}

function errorText(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason)
}
