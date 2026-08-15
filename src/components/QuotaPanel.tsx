import { AlertCircle, Gauge, RefreshCw } from 'lucide-react'
import { formatDateTime, formatShortTime, parseDate } from '../lib/time'
import type { AccountQuota, LocalQuotaWindowUsage, QuotaQueryState, QuotaWindow } from '../types'

interface QuotaPanelProps {
  state?: QuotaQueryState
  requestCount: number
  lastUsedAt: string | null
  onQuery: () => void
}

export function QuotaPanel({
  state,
  requestCount,
  lastUsedAt,
  onQuery,
}: QuotaPanelProps) {
  if (state?.status === 'loading') {
    return (
      <div className="quota-cell quota-cell-state">
        <RefreshCw className="spin" size={14} />
        <span>正在查询用量</span>
      </div>
    )
  }

  if (state?.status === 'error') {
    return (
      <div className="quota-cell quota-cell-error" data-tooltip={state.error}>
        <div className="quota-error-copy">
          <AlertCircle size={13} />
          <span>{friendlyQuotaError(state.error)}</span>
        </div>
        <button className="quota-query" onClick={onQuery} aria-label="重试查询用量">
          <RefreshCw size={12} />重试
        </button>
      </div>
    )
  }

  if (!state) {
    return (
      <div className="quota-cell quota-cell-empty">
        <div className="quota-call-meta">
          <span>{requestCount.toLocaleString()} req</span>
          <span>{lastUsedAt || '尚未调用'}</span>
        </div>
        <button className="quota-query" onClick={onQuery}>
          <Gauge size={12} />查询用量
        </button>
      </div>
    )
  }

  const windows = primaryWindows(state.quota)
  const resetCredits = resetCreditSummary(state.quota)
  return (
    <div className="quota-cell">
      <div className="quota-cell-head">
        <div className="quota-call-meta">
          <span>{requestCount.toLocaleString()} req</span>
          {state.quota.plan_type && <span className="quota-plan">{state.quota.plan_type}</span>}
        </div>
        <button
          className="quota-refresh"
          onClick={onQuery}
          data-tooltip={`刷新用量，上次查询 ${formatShortTime(state.quota.fetched_at)}`}
          aria-label="刷新用量"
        >
          <RefreshCw size={12} />
        </button>
      </div>
      {resetCredits && (
        <div className="quota-reset-credits" data-tooltip={resetCredits.tooltip}>
          重置额度 {resetCredits.count} 次
          {resetCredits.expiresAt && <span>· 最近到期 {resetCredits.expiresAt}</span>}
        </div>
      )}
      {windows.length ? (
        <div className="usage-windows">
          {windows.map((entry) => (
            <UsageWindow
              entry={entry}
              fetchedAt={state.quota.fetched_at}
              localUsage={localWindowUsage(state.quota, entry.label)}
              key={`${entry.slot}-${entry.label}`}
            />
          ))}
        </div>
      ) : (
        <div className="quota-no-window">上游未返回用量窗口</div>
      )}
    </div>
  )
}

function UsageWindow({
  entry,
  fetchedAt,
  localUsage,
}: {
  entry: QuotaWindowEntry
  fetchedAt: number | string
  localUsage: LocalQuotaWindowUsage | null
}) {
  const used = usedPercent(entry.window)
  const tone = used === null ? '' : used >= 90 ? 'critical' : used >= 70 ? 'warning' : 'healthy'
  const reset = formatResetCountdown(entry.window, fetchedAt)
  const exactReset = formatResetTime(entry.window, fetchedAt)
  const w = entry.window

  return (
    <div className="usage-window">
      <div className="usage-window-badges">
        {w.num_requests != null && (
          <span className="uw-badge">{formatCompact(w.num_requests)} req</span>
        )}
        {w.num_tokens != null && (
          <span className="uw-badge">{formatTokenCount(w.num_tokens)}</span>
        )}
        {localUsage && (
          <span
            className="uw-badge uw-badge-local"
            data-tooltip={`本机同期 ${localUsage.requests.toLocaleString()} 个成功请求、${formatTokenCount(localUsage.tokens)} Token；按当前模型 API 价目折算，仅用于成本观察，不参与 Codex 配额结算。`}
          >
            本机 API ${formatUsd(localUsage.api_equivalent_cost_usd)}
          </span>
        )}
      </div>
      <div className="usage-window-bar">
        <span className={`usage-window-label ${entry.label === '5h' ? 'short' : 'long'}`}>
          {entry.label}
        </span>
        <div className={`usage-window-track ${tone}`} aria-hidden="true">
          {used !== null && <span style={{ width: `${used}%` }} />}
        </div>
        <strong data-tooltip={used === null ? undefined : `OpenAI 官方窗口：已用 ${formatPercent(used)}，剩余 ${formatPercent(100 - used)}`}>
          {used === null ? '--' : formatPercent(used)}
        </strong>
        <span className="usage-window-reset" data-tooltip={exactReset ? `重置于 ${exactReset}` : undefined}>
          {reset || '未知'}
        </span>
      </div>
    </div>
  )
}

type WindowSlot = 'primary' | 'secondary'

interface QuotaWindowEntry {
  window: QuotaWindow
  slot: WindowSlot
  label: '5h' | '7d'
}

function primaryWindows(quota: AccountQuota): QuotaWindowEntry[] {
  const main = extractWindows(quota.rate_limit)
  if (main.length) return main
  for (const additional of quota.additional_rate_limits ?? []) {
    const windows = extractWindows(additional.rate_limit)
    if (windows.length) return windows
  }
  return []
}

function extractWindows(value: unknown): QuotaWindowEntry[] {
  const record = asRecord(value)
  if (!record) return []
  const windows = [
    { slot: 'primary' as const, window: asQuotaWindow(record.primary_window) },
    { slot: 'secondary' as const, window: asQuotaWindow(record.secondary_window) },
  ].filter((entry): entry is { slot: WindowSlot; window: QuotaWindow } => Boolean(entry.window))

  if (windows.length === 2) {
    const withDurations = windows.map((entry) => ({ ...entry, seconds: windowSeconds(entry.window) }))
    if (withDurations.every((entry) => entry.seconds !== null)) {
      return withDurations
        .sort((left, right) => left.seconds! - right.seconds!)
        .map((entry, index) => ({ ...entry, label: index === 0 ? '5h' : '7d' }))
    }
    return withDurations
      .map((entry) => ({ ...entry, label: entry.slot === 'primary' ? '7d' : '5h' } as QuotaWindowEntry))
      .sort((left, right) => left.label === '5h' ? -1 : right.label === '5h' ? 1 : 0)
  }

  return windows.map((entry) => ({
    ...entry,
    label: (windowSeconds(entry.window) ?? (entry.slot === 'primary' ? 604_800 : 18_000)) <= 21_600
      ? '5h'
      : '7d',
  }))
}

function friendlyQuotaError(error: string) {
  if (error.includes('响应解析失败')) return '上游响应格式异常'
  if (error.includes('401') || error.includes('403')) return '凭据已失效'
  if (error.includes('429')) return '请求过于频繁'
  return error
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' ? value as Record<string, unknown> : null
}

function asQuotaWindow(value: unknown): QuotaWindow | null {
  const record = asRecord(value)
  return record ? record as QuotaWindow : null
}

function usedPercent(window: QuotaWindow): number | null {
  const used = finiteNumber(window.used_percent)
  if (used !== null) return clampPercent(used)
  const remaining = finiteNumber(window.remaining_percent)
  return remaining === null ? null : clampPercent(100 - remaining)
}

function windowSeconds(window: QuotaWindow): number | null {
  const seconds = finiteNumber(window.limit_window_seconds)
  return seconds !== null && seconds > 0 ? seconds : null
}

function finiteNumber(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function localWindowUsage(quota: AccountQuota, window: '5h' | '7d') {
  return quota.local_window_usage?.find((entry) => entry.window === window) ?? null
}

function formatUsd(value: number) {
  if (!Number.isFinite(value) || value <= 0) return '0.00'
  if (value >= 100) return Math.round(value).toLocaleString()
  return value.toFixed(2)
}

function resetCreditSummary(quota: AccountQuota) {
  const resetCredits = quota.rate_limit_reset_credits
  const count = finiteNumber(resetCredits?.available_count)
  if (count === null) return null
  const expirations = (resetCredits?.credits ?? [])
    .map((credit) => credit.expires_at ? parseDate(credit.expires_at) : null)
    .filter((date): date is Date => Boolean(date && date.getTime() > Date.now()))
    .sort((left, right) => left.getTime() - right.getTime())
  const effectiveCount = resetCredits?.credits
    ? Math.min(Math.max(0, Math.trunc(count)), expirations.length)
    : Math.max(0, Math.trunc(count))
  return {
    count: effectiveCount,
    expiresAt: expirations[0] ? formatDateTime(expirations[0].getTime()) : null,
    tooltip: expirations.length
      ? expirations.map((date, index) => `第 ${index + 1} 次：${formatDateTime(date.getTime())}`).join('\n')
      : '上游未返回有效的重置额度到期时间',
  }
}

function clampPercent(value: number) {
  return Math.min(100, Math.max(0, value))
}

function formatPercent(value: number) {
  return `${Math.round(value * 10) / 10}%`
}

function formatResetCountdown(window: QuotaWindow, fetchedAt: number | string) {
  const resetAt = resetDate(window, fetchedAt)
  if (!resetAt) return null
  let minutes = Math.max(0, Math.ceil((resetAt.getTime() - Date.now()) / 60_000))
  if (minutes === 0) return '现在'
  const days = Math.floor(minutes / 1_440)
  minutes -= days * 1_440
  const hours = Math.floor(minutes / 60)
  minutes -= hours * 60
  if (days) return `${days}d ${hours}h`
  if (hours) return `${hours}h ${minutes}m`
  return `${minutes}m`
}

function formatResetTime(window: QuotaWindow, fetchedAt: number | string) {
  const date = resetDate(window, fetchedAt)
  if (!date) return null
  return formatDateTime(date.getTime())
}

function resetDate(window: QuotaWindow, fetchedAt: number | string) {
  const resetAt = parseDate(window.reset_at)
  const resetAfter = finiteNumber(window.reset_after_seconds)
  const fetchedDate = parseDate(fetchedAt)
  return resetAt ?? (resetAfter === null || !fetchedDate
    ? null
    : new Date(fetchedDate.getTime() + Math.max(0, resetAfter) * 1000))
}

function formatCompact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return String(n)
}

function formatTokenCount(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return String(n)
}
