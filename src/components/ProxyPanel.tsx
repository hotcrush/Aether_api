import { Eraser, Server } from 'lucide-react'
import type {
  Account,
  AccountQuota,
  ProxyInfo,
  QuotaQueryState,
  QuotaRateLimit,
  QuotaWindow,
  RelayUsageQueryState,
} from '../types'

interface ProxyPanelProps {
  proxy: ProxyInfo | null
  activeAccountCount: number
  accountCount: number
  accounts: Account[]
  quotaStates: Record<string, QuotaQueryState>
  relayUsageStates: Record<string, RelayUsageQueryState>
  dailyBudgetUsd: number | null
  resetBusy: boolean
  onResetCounts: () => void
  onEditDailyBudget: () => void
}

export function ProxyPanel({
  proxy,
  activeAccountCount,
  accountCount,
  accounts,
  quotaStates,
  relayUsageStates,
  dailyBudgetUsd,
  resetBusy,
  onResetCounts,
  onEditDailyBudget,
}: ProxyPanelProps) {
  const totalRequests = proxy?.total_requests ?? 0
  const totalCost = proxy?.total_cost ?? 0
  const todayCost = proxy?.today_cost ?? 0
  const totalTokens = proxy?.total_tokens ?? 0
  const inputTokens = proxy?.input_tokens ?? 0
  const outputTokens = proxy?.output_tokens ?? 0
  const cachedTokens = proxy?.cached_tokens ?? 0
  const cacheWriteTokens = proxy?.cache_write_tokens ?? 0
  const reasoningTokens = proxy?.reasoning_tokens ?? 0
  const unpricedTokens = proxy?.unpriced_tokens ?? 0
  const proxyProfileLabel = proxy?.proxy_profile === 'development' ? '开发环境' : '正式环境'
  const tokenBreakdownTitle = [
    `总计 ${totalTokens.toLocaleString()} Token`,
    `输入 ${inputTokens.toLocaleString()}`,
    `输出 ${outputTokens.toLocaleString()}`,
    `缓存读取 ${cachedTokens.toLocaleString()}`,
    `缓存写入 ${cacheWriteTokens.toLocaleString()}`,
    reasoningTokens > 0
      ? `推理 ${reasoningTokens.toLocaleString()}（已包含在输出）`
      : null,
  ].filter(Boolean).join('；')
  const pricingMeta = proxy
    ? `价格快照 ${proxy.pricing_updated_at}（${proxy.pricing_source}）。例如 Sol：输入 $5/M、缓存读取 $0.5/M、缓存写入 $6.25/M、输出 $30/M；推理 Token 已包含在输出。`
    : '内置模型价格快照'
  const costEstimateTitle = unpricedTokens > 0
    ? `按响应返回的模型及输入、输出、缓存 Token 价格估算；另有 ${unpricedTokens.toLocaleString()} Token 因模型价格未知未计入。${pricingMeta}`
    : `按响应返回的模型及输入、输出、缓存 Token 价格估算，可能与上游最终账单存在差异。${pricingMeta}`
  const quotaSummary = summarizeQuota(accounts, quotaStates, relayUsageStates)
  const quotaDisplay = dailyBudgetDisplay(dailyBudgetUsd, todayCost, quotaSummary)

  return (
    <section className="proxy-panel" aria-label="本地代理">
      <div className="proxy-overview">
        <div className="proxy-heading">
          <div className="proxy-title">
            <Server size={18} />
            本地代理
            {proxy && (
              <span className={`proxy-profile ${proxy.proxy_profile}`}>
                {proxyProfileLabel} · {proxy.port}
              </span>
            )}
          </div>
          <div className="proxy-subtitle">
            {proxy?.active_account_count ?? activeAccountCount} 个启用，共 {proxy?.account_count ?? accountCount} 个上游
          </div>
        </div>
        <div className="proxy-stats">
          <div className="stat-item">
            <span className="stat-label">累计请求</span>
            <span className="stat-value">{totalRequests.toLocaleString()}</span>
          </div>
          <div className="stat-item stat-tokens" data-tooltip={tokenBreakdownTitle}>
            <span className="stat-label">总 Tokens</span>
            <span className="stat-value">{formatTokenCount(totalTokens)}</span>
            <span className="stat-detail token-breakdown token-flow">
              <span>输入 {formatTokenCount(inputTokens)}</span>
              <span>输出 {formatTokenCount(outputTokens)}</span>
              {reasoningTokens > 0 && <span>推理 {formatTokenCount(reasoningTokens)}</span>}
            </span>
          </div>
          <div className="stat-item stat-cost" data-tooltip={costEstimateTitle}>
            <span className="stat-label stat-tooltip">估算费用</span>
            <span className="stat-value">{formatEstimatedCost(totalCost)}</span>
          </div>
          <button
            type="button"
            className="stat-item stat-quota stat-quota-button"
            onClick={onEditDailyBudget}
            data-tooltip={quotaDisplay.title}
            aria-label="设置每日 USD 额度"
          >
            <span className="stat-label stat-tooltip">每日剩余 / 总额度</span>
            <span className="stat-value">{quotaDisplay.value}</span>
            <span className="stat-detail stat-support">{quotaDisplay.detail}</span>
            {quotaDisplay.secondary && (
              <span className="stat-detail quota-secondary">{quotaDisplay.secondary}</span>
            )}
          </button>
          <button
            className="stat-reset"
            onClick={onResetCounts}
            disabled={resetBusy || (totalRequests === 0 && totalTokens === 0 && totalCost === 0)}
            data-tooltip="清空请求统计"
            aria-label="清空请求统计"
          >
            <Eraser size={15} />
          </button>
        </div>
      </div>
    </section>
  )
}

interface QuotaAggregate {
  oauthEligible: number
  oauthKnown: number
  oauthShortRemaining: number
  oauthLongKnown: number
  oauthLongRemaining: number
  relayEligible: number
  relayKnown: number
  relayRemaining: number
  relayTotal: number
}

function summarizeQuota(
  accounts: Account[],
  quotaStates: Record<string, QuotaQueryState>,
  relayUsageStates: Record<string, RelayUsageQueryState>,
): QuotaAggregate {
  const summary: QuotaAggregate = {
    oauthEligible: 0,
    oauthKnown: 0,
    oauthShortRemaining: 0,
    oauthLongKnown: 0,
    oauthLongRemaining: 0,
    relayEligible: 0,
    relayKnown: 0,
    relayRemaining: 0,
    relayTotal: 0,
  }

  for (const account of accounts) {
    if (account.status !== 'active') continue
    if (account.account_type === 'oauth') {
      summary.oauthEligible += 1
      const state = quotaStates[account.id]
      if (state?.status !== 'success') continue
      const [shortWindow, longWindow] = quotaWindows(state.quota)
      const shortRemaining = remainingPercent(shortWindow)
      if (shortRemaining !== null) {
        summary.oauthKnown += 1
        summary.oauthShortRemaining += shortRemaining / 100
      }
      const longRemaining = remainingPercent(longWindow)
      if (longRemaining !== null) {
        summary.oauthLongKnown += 1
        summary.oauthLongRemaining += longRemaining / 100
      }
      continue
    }

    summary.relayEligible += 1
    const state = relayUsageStates[account.id]
    if (state?.status !== 'success') continue
    const limit = finiteNumber(state.usage.quota_limit)
    const used = finiteNumber(state.usage.quota_used)
    const explicitRemaining = finiteNumber(state.usage.remaining)
    if (limit === null || limit <= 0 || (used === null && explicitRemaining === null)) continue
    summary.relayKnown += 1
    summary.relayTotal += limit
    summary.relayRemaining += Math.min(limit, Math.max(0, explicitRemaining ?? limit - used!))
  }
  return summary
}

function dailyBudgetDisplay(
  dailyBudgetUsd: number | null,
  todayCost: number,
  summary: QuotaAggregate,
) {
  const relayValue = summary.relayKnown > 0
    ? `${formatUsd(summary.relayRemaining)} / ${formatUsd(summary.relayTotal)}`
    : null
  const oauthValue = summary.oauthKnown > 0
    ? `${formatQuotaUnits(summary.oauthShortRemaining)} / ${summary.oauthKnown} 等效 · ${summary.oauthKnown}/${summary.oauthEligible} 已查`
    : summary.oauthEligible > 0 ? `OAuth 5h 待查询 · 0/${summary.oauthEligible}` : null
  const secondary = oauthValue ? `OAuth 5h ${oauthValue}` : relayValue ? `中转 ${relayValue}` : null
  const longDescription = summary.oauthLongKnown > 0
    ? `OAuth 7d 剩余等效 ${formatQuotaUnits(summary.oauthLongRemaining)} / ${summary.oauthLongKnown}`
    : 'OAuth 7d 暂无数据'

  if (dailyBudgetUsd !== null && dailyBudgetUsd > 0) {
    const remaining = Math.max(0, dailyBudgetUsd - todayCost)
    return {
      value: `${formatUsd(remaining)} / ${formatUsd(dailyBudgetUsd)}`,
      detail: `今日已用 ${formatUsd(todayCost)}`,
      secondary,
      title: `每日 USD 额度按本地时间统计，今日已用 ${formatUsd(todayCost)}，剩余 ${formatUsd(remaining)}。${longDescription}。点击修改每日额度。`,
    }
  }

  return {
    value: '$-- / $--',
    detail: '点击设置每日 USD 额度',
    secondary,
    title: `设置每日 USD 总额度后，将使用当天请求的实际估算费用计算剩余金额。${longDescription}。`,
  }
}

function quotaWindows(quota: AccountQuota): [QuotaWindow | null, QuotaWindow | null] {
  const limits = [
    quota.rate_limit,
    ...(quota.additional_rate_limits ?? []).map((item) => item.rate_limit),
  ]
  for (const limit of limits) {
    const windows = rateLimitWindows(limit)
    if (!windows.length) continue
    const ordered = windows.sort((left, right) => left.duration - right.duration)
    return [ordered[0]?.window ?? null, ordered[1]?.window ?? null]
  }
  return [null, null]
}

function rateLimitWindows(limit: QuotaRateLimit | null | undefined) {
  return [
    { window: limit?.primary_window, fallbackDuration: 604_800 },
    { window: limit?.secondary_window, fallbackDuration: 18_000 },
  ]
    .filter((entry): entry is { window: QuotaWindow; fallbackDuration: number } => Boolean(entry.window))
    .map((entry) => ({
      window: entry.window,
      duration: windowDuration(entry.window) ?? entry.fallbackDuration,
    }))
}

function windowDuration(window: QuotaWindow) {
  const seconds = finiteNumber(window.limit_window_seconds)
  return seconds !== null && seconds > 0 ? seconds : null
}

function remainingPercent(window: QuotaWindow | null) {
  if (!window) return null
  const remaining = finiteNumber(window.remaining_percent)
  if (remaining !== null) return clampPercent(remaining)
  const used = finiteNumber(window.used_percent)
  return used === null ? null : clampPercent(100 - used)
}

function finiteNumber(value: number | null | undefined) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function clampPercent(value: number) {
  return Math.min(100, Math.max(0, value))
}

function formatQuotaUnits(value: number) {
  return stripTrailingZeroes(value.toFixed(1))
}

function formatUsd(value: number) {
  return `$${value.toFixed(2)}`
}

function formatTokenCount(value: number) {
  if (value >= 1_000_000) return `${stripTrailingZeroes((value / 1_000_000).toFixed(2))}M`
  if (value >= 1_000) return `${stripTrailingZeroes((value / 1_000).toFixed(1))}K`
  return value.toLocaleString()
}

function formatEstimatedCost(value: number) {
  if (value <= 0) return '$0.00'
  if (value >= 0.01) return `$${value.toFixed(2)}`
  if (value >= 0.0001) return `$${value.toFixed(4)}`
  if (value >= 0.000001) return `$${value.toFixed(6)}`
  return '<$0.000001'
}

function stripTrailingZeroes(value: string) {
  return Number(value).toString()
}
