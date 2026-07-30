import { useEffect, useState } from 'react'
import { Copy, Eraser, Eye, EyeOff, RotateCcw, Server } from 'lucide-react'
import type { ProxyInfo } from '../types'

interface ProxyPanelProps {
  proxy: ProxyInfo | null
  activeAccountCount: number
  accountCount: number
  resetBusy: boolean
  resetTokenBusy: boolean
  onCopy: (value: string) => void
  onResetCounts: () => void
  onResetAccessToken: () => void
}

export function ProxyPanel({
  proxy,
  activeAccountCount,
  accountCount,
  resetBusy,
  resetTokenBusy,
  onCopy,
  onResetCounts,
  onResetAccessToken,
}: ProxyPanelProps) {
  const [keyVisible, setKeyVisible] = useState(false)
  const baseUrl = proxy ? `${proxy.base_url}/v1` : ''
  const totalRequests = proxy?.total_requests ?? 0
  const totalCost = proxy?.total_cost ?? 0
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

  useEffect(() => {
    setKeyVisible(false)
  }, [proxy?.access_token])

  return (
    <section className="proxy-panel" aria-label="本地代理">
      <div className="proxy-overview">
        <div className="proxy-heading">
          <div className="proxy-title"><Server size={18} />本地代理</div>
          <div className="proxy-subtitle">
            {proxy?.active_account_count ?? activeAccountCount} 个启用，共 {proxy?.account_count ?? accountCount} 个上游
            {proxy && <span className={`proxy-profile ${proxy.proxy_profile}`}>{proxyProfileLabel} · {proxy.port}</span>}
          </div>
        </div>
        <div className="proxy-stats">
          <div className="stat-item">
            <span className="stat-label">累计请求</span>
            <span className="stat-value">{totalRequests.toLocaleString()}</span>
          </div>
          <div className="stat-item stat-tokens" title={tokenBreakdownTitle}>
            <span className="stat-label">Tokens</span>
            <span className="stat-value">{formatTokenCount(totalTokens)}</span>
            <span className="stat-detail token-breakdown">
              <span>输入 {formatTokenCount(inputTokens)}</span>
              <span>输出 {formatTokenCount(outputTokens)}</span>
              <span>缓读 {formatTokenCount(cachedTokens)}</span>
              <span>缓写 {formatTokenCount(cacheWriteTokens)}</span>
            </span>
            {reasoningTokens > 0 && (
              <span className="stat-detail reasoning-detail">
                推理 {formatTokenCount(reasoningTokens)}（含于输出）
              </span>
            )}
          </div>
          <div className="stat-item stat-cost" title={costEstimateTitle}>
            <span className="stat-label stat-tooltip">估算费用</span>
            <span className="stat-value">{formatEstimatedCost(totalCost)}</span>
            {unpricedTokens > 0 && (
              <span className="stat-detail unpriced-note">
                另有 {formatTokenCount(unpricedTokens)} Token 未计价
              </span>
            )}
          </div>
          <button
            className="stat-reset"
            onClick={onResetCounts}
            disabled={resetBusy || (totalRequests === 0 && totalTokens === 0 && totalCost === 0)}
            title="清空请求统计"
            aria-label="清空请求统计"
          >
            <Eraser size={15} />
          </button>
        </div>
      </div>
      <div className="proxy-access">
        <div className="info-block">
          <span className="info-label">BASE URL</span>
          <div className="copy-field">
            <code>{baseUrl || '加载中'}</code>
            <button
              className="icon-btn"
              onClick={() => onCopy(baseUrl)}
              disabled={!proxy}
              title="复制 Base URL"
              aria-label="复制 Base URL"
            >
              <Copy size={15} />
            </button>
          </div>
        </div>
        <div className="info-block">
          <span className="info-label">API KEY</span>
          <div className="copy-field">
            <code>{proxy ? (keyVisible ? proxy.access_token : maskSecret(proxy.access_token)) : '加载中'}</code>
            <button
              className="icon-btn"
              onClick={() => setKeyVisible((visible) => !visible)}
              disabled={!proxy}
              title={keyVisible ? '隐藏 API Key' : '显示 API Key'}
              aria-label={keyVisible ? '隐藏 API Key' : '显示 API Key'}
            >
              {keyVisible ? <EyeOff size={15} /> : <Eye size={15} />}
            </button>
            <button
              className="icon-btn"
              onClick={() => proxy && onCopy(proxy.access_token)}
              disabled={!proxy}
              title="复制 API Key"
              aria-label="复制 API Key"
            >
              <Copy size={15} />
            </button>
            <button
              className="icon-btn key-reset"
              onClick={onResetAccessToken}
              disabled={!proxy || resetTokenBusy}
              title="重置 API Key"
              aria-label="重置 API Key"
            >
              <RotateCcw className={resetTokenBusy ? 'spin' : undefined} size={15} />
            </button>
          </div>
        </div>
      </div>
    </section>
  )
}

function maskSecret(secret: string) {
  if (!secret) return ''
  if (secret.length <= 13) return '••••••••••••'
  return `${secret.slice(0, 9)}••••••••••••${secret.slice(-4)}`
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
