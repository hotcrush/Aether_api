import { AlertCircle, Gauge, RefreshCw } from 'lucide-react'
import { formatDateTime, formatShortTime } from '../lib/time'
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
          <><AlertCircle size={13} /><span data-tooltip={state.error}>{friendlyRelayError(state.error)}</span></>
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
  const isQuotaUnit = usage.unit === 'quota' || usage.provider === 'new_api' || usage.mode === 'new_api'
  const amountUnit = isQuotaUnit ? '站点额度单位' : '美元'
  const lastRequestAt = usage.remote_last_request_at
  const requestCount = finiteCount(usage.remote_request_count)
  if (usage.provider === 'new_api' || usage.mode === 'new_api') {
    return (
      <div className="relay-usage-values">
        <div className="relay-cost-line">
          <span className="relay-cost-label">用量</span>
          <strong
            className="relay-cost-value"
            data-tooltip={amountTooltip(usage.quota_used, usage, amountUnit)}
          >
            {formatAmount(usage.quota_used, 2, true)}
          </strong>
          <button
            className="quota-refresh relay-inline-refresh"
            onClick={onQuery}
            data-tooltip={`刷新用量，上次查询 ${formatShortTime(usage.fetched_at)}`}
            aria-label="刷新中转站用量"
          >
            <RefreshCw size={11} />
          </button>
        </div>
        <div className="relay-cost-line">
          <span className="relay-cost-label">余额</span>
          <strong
            className="relay-cost-value"
            data-tooltip={usage.unlimited_quota ? undefined : amountTooltip(usage.remaining, usage, amountUnit)}
          >
            {usage.unlimited_quota ? '无限制' : formatAmount(usage.remaining, 2, true)}
          </strong>
        </div>
      </div>
    )
  }
  return (
    <div className="relay-usage-values">
      <div className="relay-cost-line">
        <span className="relay-cost-label">今日</span>
        <strong
          className="relay-cost-value"
          data-tooltip={amountTooltip(usage.today_actual_cost, usage, amountUnit)}
        >
          {formatAmount(usage.today_actual_cost, 4, isQuotaUnit)}
        </strong>
        <button
          className="quota-refresh relay-inline-refresh"
          onClick={onQuery}
          data-tooltip={`刷新用量，上次查询 ${formatShortTime(usage.fetched_at)}`}
          aria-label="刷新中转站用量"
        >
          <RefreshCw size={11} />
        </button>
      </div>
      <div className="relay-cost-line">
        <span className="relay-cost-label">近30天</span>
        <strong
          className="relay-cost-value"
          data-tooltip={amountTooltip(cost30d, usage, amountUnit)}
        >
          {formatAmount(cost30d, 4, isQuotaUnit)}
        </strong>
      </div>
      {quota ? (
        <div className="relay-quota">
          <div className="relay-cost-line">
            <span className="relay-cost-label">额度</span>
            <strong
              className={`relay-cost-value ${quota.tone}`}
              data-tooltip={`单位：${amountUnit}`}
            >
              {formatAmount(quota.used, 2, isQuotaUnit)} / {formatAmount(quota.limit, 2, isQuotaUnit)}
            </strong>
          </div>
          <div className={`relay-quota-track ${quota.tone}`}>
            <span style={{ width: `${quota.percent}%` }} />
          </div>
        </div>
      ) : usage.unlimited_quota ? (
        <div className="relay-cost-line">
          <span className="relay-cost-label">额度</span>
          <strong className="relay-cost-value">无限制</strong>
        </div>
      ) : usage.balance !== null ? (
        <div className="relay-cost-line">
          <span className="relay-cost-label">余额</span>
          <strong className="relay-cost-value">{formatAmount(usage.balance, 2, isQuotaUnit)}</strong>
        </div>
      ) : usage.remaining !== null ? (
        <div className="relay-cost-line">
          <span className="relay-cost-label">可用</span>
          <strong className="relay-cost-value">{formatAmount(usage.remaining, 2, isQuotaUnit)}</strong>
        </div>
      ) : null}
      {(requestCount !== null || usage.remote_last_model || lastRequestAt) && (
        <div className="relay-request-meta" data-tooltip="来自中转站远端日志（最多最近 1000 条）；本地 Logger 仅记录本机代理请求">
          {requestCount !== null && <span>远端请求 {requestCount.toLocaleString()}</span>}
          {usage.remote_last_model && <span>{usage.remote_last_model}</span>}
          {lastRequestAt && <span>最近 {formatDateTime(lastRequestAt)}</span>}
        </div>
      )}
      {usage.expires_at && usage.expires_at > 0 && (
        <div className="relay-unit-note">到期 {formatShortTime(usage.expires_at)}</div>
      )}
      {isQuotaUnit && usage.quota_per_unit && usage.quota_per_unit > 0 && (
        <div className="relay-unit-note">1 美元约 {usage.quota_per_unit.toLocaleString()} 单位</div>
      )}
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

function finiteCount(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? Math.floor(value)
    : null
}

function formatAmount(value: number | null, digits: number, quotaUnit: boolean) {
  const amount = finiteNumber(value)
  if (amount === null) return '-'
  if (!quotaUnit) return `$${amount.toFixed(digits)}`
  if (Math.abs(amount) >= 1_000_000) return `${(amount / 1_000_000).toFixed(2)}M`
  if (Math.abs(amount) >= 1_000) return `${(amount / 1_000).toFixed(1)}K`
  return amount.toFixed(digits)
}

function amountTooltip(
  value: number | null,
  usage: RelayUsageSummary,
  unit: string,
) {
  const amount = finiteNumber(value)
  if (amount === null) return undefined
  if (usage.unit !== 'quota' || !usage.quota_per_unit || usage.quota_per_unit <= 0) {
    return `单位：${unit}`
  }
  const approximateUsd = amount / usage.quota_per_unit
  return `${amount.toLocaleString()} ${unit}，按站点换算约 $${approximateUsd.toFixed(6)}`
}

function friendlyRelayError(error: string) {
  if (/404|405/.test(error)) return '站点未提供用量接口'
  if (/401|403/.test(error)) return 'Key 无权查询用量'
  if (/解析|JSON|format/i.test(error)) return '用量响应格式不兼容'
  return '用量读取失败'
}
