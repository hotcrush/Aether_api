import { AlertCircle, Gauge, RefreshCw } from 'lucide-react'
import type { RelayUsageQueryState, RelayUsageSummary } from '../types'

interface RelayUsagePanelProps {
  state?: RelayUsageQueryState
  requestCount: number
  lastUsedAt: string | null
  onQuery: () => void
}

export function RelayUsagePanel({
  state,
  requestCount,
  lastUsedAt,
  onQuery,
}: RelayUsagePanelProps) {
  if (state?.status === 'success') {
    return (
      <div className="relay-usage">
        <RelayUsageValues usage={state.usage} onQuery={onQuery} />
      </div>
    )
  }

  return (
    <div className="relay-usage relay-usage-empty">
      <div className="relay-usage-state">
        {state?.status === 'loading' ? (
          <><RefreshCw className="spin" size={13} /><span>正在读取用量</span></>
        ) : state?.status === 'error' ? (
          <><AlertCircle size={13} /><span title={state.error}>{friendlyRelayError(state.error)}</span></>
        ) : (
          <><Gauge size={13} /><span>{requestCount.toLocaleString()} req · {lastUsedAt || '尚未调用'}</span></>
        )}
        {state?.status !== 'loading' && (
          <button className="quota-query" onClick={onQuery}>
            <RefreshCw size={12} />{state?.status === 'error' ? '重试' : '读取用量'}
          </button>
        )}
      </div>
    </div>
  )
}

function RelayUsageValues({ usage, onQuery }: { usage: RelayUsageSummary; onQuery: () => void }) {
  const quota = quotaProgress(usage)
  const cost30d = usage.last_30_days_actual_cost ?? usage.total_actual_cost
  return (
    <div className="relay-usage-values">
      <div className="relay-cost-line">
        <span className="relay-cost-label">今日</span>
        <strong className="relay-cost-value">{formatUsd(usage.today_actual_cost, 4)}</strong>
        <button
          className="quota-refresh relay-inline-refresh"
          onClick={onQuery}
          title={`刷新用量，上次查询 ${formatLocalTime(usage.fetched_at)}`}
          aria-label="刷新中转站用量"
        >
          <RefreshCw size={11} />
        </button>
      </div>
      <div className="relay-cost-line">
        <span className="relay-cost-label">近30天</span>
        <strong className="relay-cost-value">{formatUsd(cost30d, 4)}</strong>
      </div>
      {quota ? (
        <div className="relay-quota">
          <div className="relay-cost-line">
            <span className="relay-cost-label">额度</span>
            <strong className={`relay-cost-value ${quota.tone}`}>{formatUsd(quota.used, 2)} / {formatUsd(quota.limit, 2)}</strong>
          </div>
          <div className={`relay-quota-track ${quota.tone}`}>
            <span style={{ width: `${quota.percent}%` }} />
          </div>
        </div>
      ) : usage.balance !== null ? (
        <div className="relay-cost-line">
          <span className="relay-cost-label">余额</span>
          <strong className="relay-cost-value">{formatUsd(usage.balance, 2)}</strong>
        </div>
      ) : usage.remaining !== null ? (
        <div className="relay-cost-line">
          <span className="relay-cost-label">可用</span>
          <strong className="relay-cost-value">{formatUsd(usage.remaining, 2)}</strong>
        </div>
      ) : null}
    </div>
  )
}

function quotaProgress(usage: RelayUsageSummary) {
  const used = finiteNumber(usage.quota_used)
  const limit = finiteNumber(usage.quota_limit)
  if (used === null || limit === null || limit <= 0) return null
  const percent = Math.min(100, Math.max(0, used / limit * 100))
  return {
    used,
    limit,
    percent,
    tone: percent >= 100 ? 'critical' : percent >= 80 ? 'warning' : 'healthy',
  }
}

function finiteNumber(value: number | null) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function formatUsd(value: number | null, digits: number) {
  const amount = finiteNumber(value)
  return amount === null ? '-' : `$${amount.toFixed(digits)}`
}

function formatLocalTime(value: number | string) {
  const numeric = typeof value === 'number' ? value : Number(value)
  const date = Number.isFinite(numeric)
    ? new Date(numeric < 1_000_000_000_000 ? numeric * 1000 : numeric)
    : new Date(value)
  return Number.isNaN(date.getTime()) ? '未知' : date.toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  })
}

function friendlyRelayError(error: string) {
  if (/404|405/.test(error)) return '站点未提供用量接口'
  if (/401|403/.test(error)) return 'Key 无权查询用量'
  if (/解析|JSON|format/i.test(error)) return '用量响应格式不兼容'
  return '用量读取失败'
}
