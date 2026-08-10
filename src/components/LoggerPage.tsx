import { ArrowRight, BadgeAlert, Eraser, RefreshCw, Search, ScrollText } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { clearRequestLogs, listRequestLogs } from '../lib/commands'
import { errorText } from '../lib/format'
import { formatTime, parseLogTime } from '../lib/time'
import type { RequestLog, RequestLogQuery, RequestLogStatus } from '../types'

const PAGE_SIZE = 100
const AUTO_REFRESH_INTERVAL_MS = 2_000
const SEARCH_DEBOUNCE_MS = 280
const CLEAR_CONFIRM_TIMEOUT_MS = 4_000

type StatusFilter = RequestLogStatus | 'all'
type LoadMode = 'initial' | 'manual' | 'auto'

const STATUS_FILTERS: Array<{ value: StatusFilter; label: string }> = [
  { value: 'all', label: '全部' },
  { value: 'pending', label: '进行中' },
  { value: 'success', label: '成功' },
  { value: 'retry', label: '重试' },
  { value: 'error', label: '失败' },
  { value: 'cancelled', label: '已取消' },
]

const STATUS_LABELS: Record<RequestLogStatus, string> = {
  pending: '进行中',
  success: '成功',
  retry: '已重试',
  error: '失败',
  cancelled: '已取消',
}

export function LoggerPage() {
  const [items, setItems] = useState<RequestLog[]>([])
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all')
  const [searchInput, setSearchInput] = useState('')
  const [search, setSearch] = useState('')
  const [modelMismatchOnly, setModelMismatchOnly] = useState(false)
  const [nextBeforeId, setNextBeforeId] = useState<number | null>(null)
  const [hasMore, setHasMore] = useState(false)
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [clearBusy, setClearBusy] = useState(false)
  const [clearArmed, setClearArmed] = useState(false)
  const [browsingHistory, setBrowsingHistory] = useState(false)
  const [loadError, setLoadError] = useState('')
  const [feedback, setFeedback] = useState('')
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null)

  const mountedRef = useRef(true)
  const requestSerialRef = useRef(0)
  const activeRequestRef = useRef<number | null>(null)
  const browsingHistoryRef = useRef(false)
  const clearTimerRef = useRef<number | null>(null)

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setSearch(searchInput.trim())
    }, SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(timer)
  }, [searchInput])

  const disarmClear = useCallback(() => {
    if (clearTimerRef.current !== null) {
      window.clearTimeout(clearTimerRef.current)
      clearTimerRef.current = null
    }
    setClearArmed(false)
  }, [])

  useEffect(() => () => {
    mountedRef.current = false
    requestSerialRef.current += 1
    if (clearTimerRef.current !== null) window.clearTimeout(clearTimerRef.current)
  }, [])

  const baseQuery = useMemo<RequestLogQuery>(() => ({
    status: statusFilter === 'all' ? undefined : statusFilter,
    search: search || undefined,
    model_mismatch_only: modelMismatchOnly,
    limit: PAGE_SIZE,
  }), [modelMismatchOnly, search, statusFilter])

  const loadFirstPage = useCallback(async (mode: LoadMode) => {
    if (mode === 'auto' && activeRequestRef.current !== null) return

    const requestSerial = ++requestSerialRef.current
    activeRequestRef.current = requestSerial
    if (mode === 'initial') {
      setLoading(true)
      setRefreshing(false)
    } else if (mode === 'manual') {
      setLoading(false)
      setRefreshing(true)
    }

    try {
      const page = await listRequestLogs(baseQuery)
      if (!mountedRef.current || requestSerial !== requestSerialRef.current) return
      setItems(page.items)
      setHasMore(page.has_more)
      setNextBeforeId(page.next_before_id)
      setLoadError('')
      setFeedback('')
      setLastUpdatedAt(Date.now())
      if (mode !== 'auto') {
        browsingHistoryRef.current = false
        setBrowsingHistory(false)
      }
    } catch (error) {
      if (!mountedRef.current || requestSerial !== requestSerialRef.current) return
      setLoadError(errorText(error))
    } finally {
      if (mountedRef.current && activeRequestRef.current === requestSerial) {
        activeRequestRef.current = null
        setLoading(false)
        setRefreshing(false)
      }
    }
  }, [baseQuery])

  useEffect(() => {
    browsingHistoryRef.current = false
    setBrowsingHistory(false)
    setItems([])
    setHasMore(false)
    setNextBeforeId(null)
    void loadFirstPage('initial')
  }, [loadFirstPage])

  useEffect(() => {
    if (browsingHistory) return
    const refresh = () => {
      if (document.visibilityState === 'visible' && !browsingHistoryRef.current) {
        void loadFirstPage('auto')
      }
    }
    const timer = window.setInterval(refresh, AUTO_REFRESH_INTERVAL_MS)
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') refresh()
    }
    document.addEventListener('visibilitychange', refreshWhenVisible)
    window.addEventListener('focus', refresh)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      window.removeEventListener('focus', refresh)
    }
  }, [browsingHistory, loadFirstPage])

  const refreshNow = () => {
    browsingHistoryRef.current = false
    setBrowsingHistory(false)
    void loadFirstPage('manual')
  }

  const loadMore = async () => {
    if (nextBeforeId === null || loadingMore || clearBusy) return
    browsingHistoryRef.current = true
    setBrowsingHistory(true)
    const requestSerial = ++requestSerialRef.current
    activeRequestRef.current = requestSerial
    setLoadingMore(true)
    setRefreshing(false)
    try {
      const page = await listRequestLogs({ ...baseQuery, before_id: nextBeforeId })
      if (!mountedRef.current || requestSerial !== requestSerialRef.current) return
      setItems((current) => mergeRequestLogs(current, page.items))
      setHasMore(page.has_more)
      setNextBeforeId(page.next_before_id)
      setLoadError('')
    } catch (error) {
      if (!mountedRef.current || requestSerial !== requestSerialRef.current) return
      setLoadError(errorText(error))
    } finally {
      if (mountedRef.current && activeRequestRef.current === requestSerial) {
        activeRequestRef.current = null
        setLoadingMore(false)
      }
    }
  }

  const handleClear = async () => {
    if (!clearArmed) {
      setClearArmed(true)
      setFeedback('再次点击“确认清空”删除所有请求日志')
      clearTimerRef.current = window.setTimeout(() => {
        clearTimerRef.current = null
        if (mountedRef.current) {
          setClearArmed(false)
          setFeedback('')
        }
      }, CLEAR_CONFIRM_TIMEOUT_MS)
      return
    }

    disarmClear()
    const requestSerial = ++requestSerialRef.current
    activeRequestRef.current = requestSerial
    setClearBusy(true)
    setLoadError('')
    try {
      const deleted = await clearRequestLogs()
      if (!mountedRef.current || requestSerial !== requestSerialRef.current) return
      setItems([])
      setHasMore(false)
      setNextBeforeId(null)
      browsingHistoryRef.current = false
      setBrowsingHistory(false)
      setFeedback(`已清空 ${deleted.toLocaleString()} 条请求日志`)
      setLastUpdatedAt(Date.now())
    } catch (error) {
      if (!mountedRef.current || requestSerial !== requestSerialRef.current) return
      setLoadError(errorText(error))
      setFeedback('')
    } finally {
      if (mountedRef.current && activeRequestRef.current === requestSerial) {
        activeRequestRef.current = null
        setClearBusy(false)
      }
    }
  }

  const requestCount = useMemo(
    () => new Set(items.map((item) => item.request_id)).size,
    [items],
  )
  const controlsBusy = loading || refreshing || loadingMore || clearBusy

  return (
    <main className="logger-page">
      <section className="logger-panel" aria-label="Logger 请求日志">
        <div className="logger-head">
          <div className="logger-heading">
            <div className="logger-title"><ScrollText size={18} />Logger</div>
            <div className="logger-subtitle">
              {items.length.toLocaleString()} 次尝试 · {requestCount.toLocaleString()} 个请求
              {lastUpdatedAt !== null && ` · 更新于 ${formatTime(lastUpdatedAt)}`}
            </div>
          </div>
          <div className="logger-head-actions">
            <span className={`logger-live-state${browsingHistory ? ' paused' : ''}`}>
              <span aria-hidden="true" />
              {browsingHistory ? '浏览历史' : '实时刷新'}
            </span>
            <button
              className="btn logger-refresh"
              type="button"
              onClick={refreshNow}
              disabled={controlsBusy}
              data-tooltip={browsingHistory ? '返回最新日志' : '立即刷新'}
            >
              <RefreshCw className={refreshing ? 'spin' : undefined} size={15} />
              {browsingHistory ? '回到最新' : '刷新'}
            </button>
            <button
              className={`btn logger-clear${clearArmed ? ' armed' : ''}`}
              type="button"
              onClick={() => void handleClear()}
              disabled={clearBusy || loading}
              data-tooltip={clearArmed ? '确认删除所有请求日志' : '清空请求日志'}
            >
              <Eraser size={15} />
              {clearBusy ? '清空中' : clearArmed ? '确认清空' : '清空'}
            </button>
          </div>
        </div>

        <div className="logger-toolbar">
          <label className="logger-search">
            <Search size={15} aria-hidden="true" />
            <input
              type="search"
              value={searchInput}
              onChange={(event) => {
                disarmClear()
                setFeedback('')
                setSearchInput(event.target.value)
              }}
              placeholder="搜索请求 ID、上游、模型、路径或错误"
              aria-label="搜索请求日志"
            />
          </label>
          <div className="logger-filter-group">
            <div className="logger-status-filters" role="group" aria-label="按日志状态筛选">
              {STATUS_FILTERS.map((option) => (
                <button
                  key={option.value}
                  className={statusFilter === option.value ? 'active' : ''}
                  type="button"
                  aria-pressed={statusFilter === option.value}
                  onClick={() => {
                    disarmClear()
                    setFeedback('')
                    setStatusFilter(option.value)
                  }}
                >
                  {option.label}
                </button>
              ))}
            </div>
            <button
              className={`btn logger-model-filter${modelMismatchOnly ? ' active' : ''}`}
              type="button"
              aria-pressed={modelMismatchOnly}
              data-tooltip="仅显示上游响应模型与请求模型不一致的日志"
              onClick={() => {
                disarmClear()
                setFeedback('')
                setModelMismatchOnly((current) => !current)
              }}
            >
              <BadgeAlert size={14} />
              模型不一致
            </button>
          </div>
        </div>

        {(loadError || feedback) && (
          <div
            className={`logger-feedback${loadError ? ' error' : ''}`}
            role={loadError ? 'alert' : 'status'}
            aria-live="polite"
          >
            {loadError || feedback}
          </div>
        )}

        <div className="logger-list" aria-label="请求日志列表">
          <div className="logger-list-head" aria-hidden="true">
            <span>时间</span>
            <span>请求</span>
            <span>命中上游</span>
            <span>结果</span>
            <span>性能</span>
            <span>Tokens / 费用</span>
          </div>

          {loading && items.length === 0 ? (
            <LoggerState icon={<RefreshCw className="spin" size={20} />} title="正在读取请求日志" />
          ) : items.length === 0 ? (
            <LoggerState
              icon={<ScrollText size={22} />}
              title={loadError ? '日志加载失败' : '暂无匹配的请求日志'}
              detail={loadError ? '可手动刷新重试' : '新的代理请求会自动出现在这里'}
            />
          ) : (
            <ol className="logger-items">
              {items.map((item) => <LoggerRow key={item.id} item={item} />)}
            </ol>
          )}
        </div>

        {(hasMore || loadingMore || browsingHistory) && items.length > 0 && (
          <div className="logger-pagination">
            <span>{browsingHistory ? '浏览历史时实时刷新已暂停' : '仅显示最新日志'}</span>
            {hasMore && (
              <button
                className="btn"
                type="button"
                onClick={() => void loadMore()}
                disabled={loadingMore || clearBusy}
              >
                <RefreshCw className={loadingMore ? 'spin' : undefined} size={14} />
                {loadingMore ? '加载中' : '加载更多'}
              </button>
            )}
          </div>
        )}
      </section>
    </main>
  )
}

function LoggerRow({ item }: { item: RequestLog }) {
  const time = parseLogTime(item.created_at)
  const accountName = item.account_name || (item.account_id ? '未命名上游' : '本地路由')
  const accountType = item.account_type === 'oauth'
    ? 'OAuth'
    : item.account_type === 'api_key'
      ? '中转站'
      : item.source || '本地'
  const tokenTitle = [
    `输入 ${item.input_tokens.toLocaleString()}`,
    `输出 ${item.output_tokens.toLocaleString()}`,
    `缓存读取 ${item.cached_tokens.toLocaleString()}`,
    `缓存写入 ${item.cache_write_tokens.toLocaleString()}`,
    item.reasoning_tokens > 0 ? `推理 ${item.reasoning_tokens.toLocaleString()}` : null,
    item.unpriced_tokens > 0 ? `未计价 ${item.unpriced_tokens.toLocaleString()}` : null,
  ].filter(Boolean).join('；')

  return (
    <li className={`logger-entry status-${item.status}`}>
      <div className="logger-row">
        <div className="logger-time" data-tooltip={time.full}>
          <time dateTime={item.created_at}>{time.clock}</time>
          <span>{time.date}</span>
        </div>

        <div className="logger-request">
          <div className="logger-request-line">
            <span className="logger-method">{item.method || '—'}</span>
            <code data-tooltip={item.path}>{item.path || '未知路径'}</code>
          </div>
          <div className="logger-request-meta">
            <span>{endpointLabel(item.endpoint_family)}</span>
            <span data-tooltip="传输协议">{transportLabel(item.transport)}</span>
            <span data-tooltip="出站路径">{outboundProxyLabel(item.outbound_proxy)}</span>
            {item.model && <span data-tooltip={item.model}>{item.model}</span>}
            {item.upstream_response_model && (
              <span
                className={`logger-model-audit${item.model_mismatch ? ' mismatch' : ''}`}
                data-tooltip={item.model_mismatch
                  ? `请求模型 ${item.model || '未知'}，上游响应声明 ${item.upstream_response_model}`
                  : `上游响应声明 ${item.upstream_response_model}`}
              >
                <ArrowRight size={11} aria-hidden="true" />
                {item.upstream_response_model}
              </span>
            )}
            <span>尝试 #{Math.max(0, item.attempt_index)}</span>
            <span className="logger-request-id" data-tooltip={item.request_id}>{shortRequestId(item.request_id)}</span>
          </div>
        </div>

        <div className="logger-upstream" data-tooltip={item.account_id || undefined}>
          <strong>{accountName}</strong>
          <span>{accountType}</span>
        </div>

        <div className="logger-result">
          <span className={`logger-status status-${item.status}`}>
            <span aria-hidden="true" />
            {STATUS_LABELS[item.status]}
          </span>
          <span className="logger-http-status">
            {item.http_status === null ? 'HTTP —' : `HTTP ${item.http_status}`}
          </span>
        </div>

        <div className="logger-performance">
          <span data-tooltip="首字节时间">TTFB {formatDuration(item.ttfb_ms)}</span>
          <span data-tooltip="总耗时">总计 {formatDuration(item.duration_ms)}</span>
        </div>

        <div className="logger-usage" data-tooltip={tokenTitle || undefined}>
          <strong>{item.total_tokens > 0 ? formatCompactNumber(item.total_tokens) : '—'} Token</strong>
          <span>{formatEstimatedCost(item.estimated_cost)}</span>
        </div>
      </div>
      {item.message && (
        <div className="logger-message" data-tooltip={item.message}>{item.message}</div>
      )}
    </li>
  )
}

function LoggerState({
  icon,
  title,
  detail,
}: {
  icon: React.ReactNode
  title: string
  detail?: string
}) {
  return (
    <div className="logger-state">
      {icon}
      <strong>{title}</strong>
      {detail && <span>{detail}</span>}
    </div>
  )
}

function mergeRequestLogs(current: RequestLog[], incoming: RequestLog[]) {
  const logs = new Map<number, RequestLog>()
  for (const item of current) logs.set(item.id, item)
  for (const item of incoming) logs.set(item.id, item)
  return [...logs.values()].sort((left, right) => right.id - left.id)
}

function formatDuration(value: number | null) {
  if (value === null || !Number.isFinite(value)) return '—'
  if (value < 1_000) return `${Math.max(0, Math.round(value))} ms`
  if (value < 60_000) return `${(value / 1_000).toFixed(value < 10_000 ? 2 : 1)} s`
  return `${(value / 60_000).toFixed(1)} min`
}

function formatCompactNumber(value: number) {
  if (value < 1_000) return value.toLocaleString()
  if (value < 1_000_000) return `${trimTrailingZero((value / 1_000).toFixed(1))}K`
  return `${trimTrailingZero((value / 1_000_000).toFixed(1))}M`
}

function formatEstimatedCost(value: number) {
  if (!Number.isFinite(value) || value <= 0) return '$0.00'
  if (value < 0.01) return `$${value.toFixed(6)}`
  return `$${value.toFixed(4)}`
}

function trimTrailingZero(value: string) {
  return value.endsWith('.0') ? value.slice(0, -2) : value
}

function endpointLabel(value: string) {
  switch (value) {
    case 'responses': return 'Responses'
    case 'models': return 'Models'
    case 'chat_completions': return 'Chat Completions'
    default: return value || 'Other'
  }
}

function transportLabel(value: RequestLog['transport']) {
  switch (value) {
    case 'websocket': return 'WebSocket'
    case 'sse': return 'HTTP SSE'
    case 'http': return 'HTTP'
    default: return '传输未知'
  }
}

function outboundProxyLabel(value: RequestLog['outbound_proxy']) {
  switch (value) {
    case 'direct': return '直连'
    case 'http': return 'HTTP 代理'
    case 'http_connect': return 'HTTP CONNECT'
    case 'https': return 'HTTPS 代理'
    case 'socks5': return 'SOCKS5'
    case 'socks5h': return 'SOCKS5H'
    default: return '代理未知'
  }
}

function shortRequestId(value: string) {
  if (!value) return '无请求 ID'
  if (value.length <= 12) return value
  return `${value.slice(0, 8)}…${value.slice(-4)}`
}
