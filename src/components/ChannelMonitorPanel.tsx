import { useMemo, useState, type ReactNode } from 'react'
import {
  Activity,
  AlertCircle,
  CheckCircle2,
  Clock3,
  Gauge,
  Radio,
  RefreshCw,
  Search,
  Server,
  ShieldAlert,
  ShieldCheck,
  XCircle,
  Zap,
} from 'lucide-react'
import { formatRelativeTime, formatTime } from '../lib/time'
import type {
  ChannelMonitorEvent,
  ChannelMonitorItem,
  ChannelMonitorSnapshot,
  ChannelMonitorStatus,
  ChannelMonitorWindow,
  ModelIntegrityResult,
} from '../monitorTypes'
import { Dialog } from './Dialog'

interface ChannelMonitorPanelProps {
  snapshot: ChannelMonitorSnapshot | null
  loading: boolean
  refreshing: boolean
  error: string
  probeBusy: Set<string>
  integrityProbeBusy: Set<string>
  onRefresh: () => void
  onProbe: (accountId: string) => void
  onIntegrityProbe: (accountId: string, model: string) => Promise<ModelIntegrityResult | null>
}

type MonitorFilter = 'all' | 'healthy' | 'abnormal' | 'unknown'

export function ChannelMonitorPanel({
  snapshot,
  loading,
  refreshing,
  error,
  probeBusy,
  integrityProbeBusy,
  onRefresh,
  onProbe,
  onIntegrityProbe,
}: ChannelMonitorPanelProps) {
  const [window, setWindow] = useState<ChannelMonitorWindow>('24h')
  const [filter, setFilter] = useState<MonitorFilter>('all')
  const [query, setQuery] = useState('')
  const [detail, setDetail] = useState<ChannelMonitorItem | null>(null)
  const [integrityTarget, setIntegrityTarget] = useState<ChannelMonitorItem | null>(null)
  const [integrityModel, setIntegrityModel] = useState('')
  const [integrityResult, setIntegrityResult] = useState<ModelIntegrityResult | null>(null)

  const openIntegrityProbe = (item: ChannelMonitorItem) => {
    const configuredModel = item.models.find((model) => model !== '*' && !model.includes('*'))
    setIntegrityTarget(item)
    setIntegrityModel(item.integrity?.requested_model || configuredModel || item.timeline[0]?.model || '')
    setIntegrityResult(item.integrity)
  }

  const runIntegrityProbe = async () => {
    if (!integrityTarget || !integrityModel.trim()) return
    const result = await onIntegrityProbe(integrityTarget.account_id, integrityModel.trim())
    if (result) setIntegrityResult(result)
  }

  const items = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase()
    return (snapshot?.items ?? []).filter((item) => {
      if (normalizedQuery && ![
        item.name,
        item.account_type,
        item.timeline[0]?.model,
        item.timeline[0]?.endpoint_family,
      ].some((value) => value?.toLowerCase().includes(normalizedQuery))) return false
      const status = displayStatus(item)
      if (filter === 'healthy') return status === 'operational'
      if (filter === 'abnormal') return status === 'degraded' || status === 'failed' || status === 'error'
      if (filter === 'unknown') return status === null
      return true
    })
  }, [snapshot, filter, query])

  if (loading && !snapshot) {
    return (
      <section className="monitor-dashboard monitor-loading" aria-busy="true">
        <RefreshCw className="spin" size={22} />
        <span>正在汇总渠道状态</span>
      </section>
    )
  }

  return (
    <section className="monitor-dashboard">
      <header className="monitor-hero">
        <div className="monitor-hero-copy">
          <span className="monitor-eyebrow"><Radio size={13} />本地实时观测</span>
          <h2>渠道监控</h2>
          <p>常规统计不主动请求；模型验真仅在手动触发时发送三组低成本探针。</p>
        </div>
        <div className="monitor-hero-actions">
          <span className="monitor-updated">
            {snapshot ? `更新于 ${formatTime(snapshot.generated_at)}` : '等待监控数据'}
          </span>
          <button
            className="btn"
            onClick={onRefresh}
            disabled={refreshing}
            aria-label="刷新渠道监控"
          >
            <RefreshCw className={refreshing ? 'spin' : undefined} size={14} />
            刷新
          </button>
        </div>
      </header>

      {error && (
        <div className="monitor-error" role="alert">
          <AlertCircle size={15} />
          <span>{error}</span>
          <button onClick={onRefresh}>重试</button>
        </div>
      )}

      <div className="monitor-summary">
        <SummaryCard
          icon={<Server size={17} />}
          label="启用渠道"
          value={String(snapshot?.active_channels ?? 0)}
          detail={(snapshot?.abnormal_channels ?? 0) > 0
            ? `${snapshot?.abnormal_channels} 个异常`
            : '当前无异常'}
          tone={(snapshot?.abnormal_channels ?? 0) > 0 ? 'warning' : 'healthy'}
        />
        <SummaryCard
          icon={<CheckCircle2 size={17} />}
          label="24h 可用率"
          value={formatPercent(snapshot?.availability_24h)}
          detail={`${formatNumber(snapshot?.available_24h)} / ${formatNumber(snapshot?.total_24h)} 次可用`}
          tone={availabilityTone(snapshot?.availability_24h)}
        />
        <SummaryCard
          icon={<Zap size={17} />}
          label="平均首包"
          value={formatLatency(snapshot?.avg_ttfb_24h_ms)}
          detail="过去 24 小时"
        />
        <SummaryCard
          icon={<XCircle size={17} />}
          label="失败尝试"
          value={formatNumber(snapshot?.failed_24h)}
          detail="包括故障切换前的失败"
          tone={(snapshot?.failed_24h ?? 0) > 0 ? 'danger' : 'neutral'}
        />
      </div>

      <div className="monitor-toolbar">
        <div className="monitor-window" aria-label="统计窗口">
          {(['24h', '7d'] as const).map((value) => (
            <button
              className={window === value ? 'active' : ''}
              onClick={() => setWindow(value)}
              key={value}
            >
              {value === '24h' ? '24 小时' : '7 天'}
            </button>
          ))}
        </div>
        <label className="monitor-search">
          <Search size={14} />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索渠道或模型"
          />
        </label>
        <select
          className="monitor-filter"
          value={filter}
          onChange={(event) => setFilter(event.target.value as MonitorFilter)}
          aria-label="筛选渠道状态"
        >
          <option value="all">全部状态</option>
          <option value="healthy">可用</option>
          <option value="abnormal">异常</option>
          <option value="unknown">暂无数据</option>
        </select>
      </div>

      {!snapshot?.items.length ? (
        <div className="monitor-empty">
          <Activity size={28} />
          <strong>暂无可监控的上游</strong>
          <span>导入 OAuth 账号或添加中转站后会自动出现在这里。</span>
        </div>
      ) : !items.length ? (
        <div className="monitor-empty compact">
          <Search size={24} />
          <strong>没有匹配的渠道</strong>
          <span>尝试清除搜索内容或切换状态筛选。</span>
        </div>
      ) : (
        <div className="monitor-grid">
          {items.map((item) => (
            <ChannelCard
              item={item}
              window={window}
              probing={probeBusy.has(item.account_id)}
              integrityProbing={integrityProbeBusy.has(item.account_id)}
              onProbe={() => onProbe(item.account_id)}
              onIntegrityProbe={() => openIntegrityProbe(item)}
              onDetail={() => setDetail(item)}
              key={item.account_id}
            />
          ))}
        </div>
      )}

      <MonitorDetailDialog item={detail} onClose={() => setDetail(null)} />
      <IntegrityProbeDialog
        item={integrityTarget}
        model={integrityModel}
        result={integrityResult}
        busy={Boolean(integrityTarget && integrityProbeBusy.has(integrityTarget.account_id))}
        onModelChange={setIntegrityModel}
        onRun={() => { void runIntegrityProbe() }}
        onClose={() => setIntegrityTarget(null)}
      />
    </section>
  )
}

function SummaryCard({
  icon,
  label,
  value,
  detail,
  tone = 'neutral',
}: {
  icon: ReactNode
  label: string
  value: string
  detail: string
  tone?: 'healthy' | 'warning' | 'danger' | 'neutral'
}) {
  return (
    <div className={`monitor-summary-card ${tone}`}>
      <span className="monitor-summary-icon">{icon}</span>
      <span className="monitor-summary-label">{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  )
}

function ChannelCard({
  item,
  window,
  probing,
  integrityProbing,
  onProbe,
  onIntegrityProbe,
  onDetail,
}: {
  item: ChannelMonitorItem
  window: ChannelMonitorWindow
  probing: boolean
  integrityProbing: boolean
  onProbe: () => void
  onIntegrityProbe: () => void
  onDetail: () => void
}) {
  const status = displayStatus(item)
  const availability = window === '24h' ? item.availability_24h : item.availability_7d
  const latency = window === '24h' ? item.avg_ttfb_24h_ms : item.avg_ttfb_7d_ms
  const attempts = window === '24h' ? item.attempts_24h : item.attempts_7d
  const failures = window === '24h' ? item.failed_24h : item.failed_7d
  const cost = window === '24h' ? item.estimated_cost_24h : item.estimated_cost_7d
  const latest = item.timeline[0]

  return (
    <article className={`monitor-channel-card ${status ?? 'unknown'}`}>
      <div className="monitor-channel-head">
        <div className="monitor-channel-title">
          <span className={`monitor-state-dot ${status ?? 'unknown'}`} />
          <div>
            <h3 data-tooltip={item.name}>{item.name || '未命名上游'}</h3>
            <div className="monitor-channel-meta">
              <span>{item.account_type === 'oauth' ? 'OAuth' : '中转站'}</span>
              <span>·</span>
              <span>{item.account_status === 'disabled' ? '已停用' : statusLabel(status)}</span>
            </div>
          </div>
        </div>
        <div className="monitor-channel-actions">
          {item.account_type === 'api_key' && (
            <button
              className="monitor-probe monitor-integrity-trigger"
              onClick={onIntegrityProbe}
              disabled={item.account_status === 'disabled' || integrityProbing}
              data-tooltip="用三组动态探针检查模型声明与能力指纹"
            >
              {integrityProbing
                ? <RefreshCw className="spin" size={13} />
                : <ShieldCheck size={13} />}
              验模
            </button>
          )}
          <button
            className="monitor-probe"
            onClick={onProbe}
            disabled={item.account_status === 'disabled' || probing}
            data-tooltip={item.account_status === 'disabled' ? '渠道已停用' : '执行一次轻量连接检测'}
          >
            <RefreshCw className={probing ? 'spin' : undefined} size={13} />
            检测
          </button>
        </div>
      </div>

      <div className="monitor-channel-metrics">
        <Metric label="可用率" value={formatPercent(availability)} tone={availabilityTone(availability)} />
        <Metric label="平均首包" value={formatLatency(latency)} />
        <Metric label="尝试" value={formatNumber(attempts)} />
        <Metric label="失败" value={formatNumber(failures)} tone={failures > 0 ? 'danger' : 'neutral'} />
      </div>

      <div className="monitor-capacity-line">
        <span><Gauge size={12} />容量</span>
        <div className="monitor-capacity-track" aria-hidden="true">
          <span style={{ width: `${capacityPercent(item)}%` }} />
        </div>
        <strong>{item.current_capacity} / {item.concurrency}</strong>
      </div>

      {item.account_type === 'api_key' && item.integrity && (
        <IntegritySummary result={item.integrity} onClick={onIntegrityProbe} />
      )}

      <Timeline events={item.timeline} />

      <div className="monitor-latest">
        <div>
          <Clock3 size={12} />
          <span>{item.latest_checked_at ? formatRelativeTime(item.latest_checked_at) : '尚无观测数据'}</span>
          {latest?.source === 'probe' && <span className="monitor-source">手动检测</span>}
        </div>
        {latest && (
          <span className="monitor-route" data-tooltip={[latest.endpoint_family, latest.model].filter(Boolean).join(' · ')}>
            {[latest.endpoint_family, latest.model].filter(Boolean).join(' · ') || '上游请求'}
          </span>
        )}
      </div>

      {latest?.message && (
        <div className="monitor-message" data-tooltip={latest.message}>
          <AlertCircle size={12} />{latest.message}
        </div>
      )}

      <button className="monitor-detail-link" onClick={onDetail} disabled={!item.timeline.length}>
        查看最近观测{cost !== null && cost !== undefined ? ` · 估算 ${formatUsd(cost)}` : ''}
      </button>
    </article>
  )
}

function Metric({
  label,
  value,
  tone = 'neutral',
}: {
  label: string
  value: string
  tone?: 'healthy' | 'warning' | 'danger' | 'neutral'
}) {
  return (
    <div className={`monitor-metric ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  )
}

function Timeline({ events }: { events: ChannelMonitorEvent[] }) {
  const points = [...events].reverse()
  return (
    <div className="monitor-timeline-wrap">
      <div className="monitor-timeline-label">
        <span>最近观测</span>
        <span>旧 → 新</span>
      </div>
      <div className="monitor-timeline" aria-label="最近渠道状态">
        {points.length ? points.map((event) => (
          <span
            className={event.status}
            data-tooltip={`${statusLabel(event.status)} · ${formatLatency(event.ttfb_ms)} · ${formatTime(event.created_at)}`}
            key={event.id}
          />
        )) : <span className="unknown wide" data-tooltip="暂无数据" />}
      </div>
    </div>
  )
}

function IntegritySummary({
  result,
  onClick,
}: {
  result: ModelIntegrityResult
  onClick: () => void
}) {
  const highRisk = result.risk === 'high_risk' || result.risk === 'suspicious'
  return (
    <button
      className={`monitor-integrity-summary ${result.risk}`}
      onClick={onClick}
      data-tooltip={result.summary}
    >
      {highRisk ? <ShieldAlert size={14} /> : <ShieldCheck size={14} />}
      <span>
        <strong>{integrityRiskLabel(result.risk)}</strong>
        <small>{result.requested_model} · {formatRelativeTime(result.created_at)}</small>
      </span>
      <b>{result.score}</b>
    </button>
  )
}

function IntegrityProbeDialog({
  item,
  model,
  result,
  busy,
  onModelChange,
  onRun,
  onClose,
}: {
  item: ChannelMonitorItem | null
  model: string
  result: ModelIntegrityResult | null
  busy: boolean
  onModelChange: (model: string) => void
  onRun: () => void
  onClose: () => void
}) {
  const modelListId = item ? `integrity-models-${item.account_id}` : 'integrity-models'
  return (
    <Dialog
      open={Boolean(item)}
      title={item ? `${item.name} · 模型验真` : '模型验真'}
      onClose={onClose}
      preventClose={busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>关闭</button>
          <button
            className="btn btn-primary"
            onClick={onRun}
            disabled={busy || !model.trim()}
          >
            {busy ? <RefreshCw className="spin" size={14} /> : <ShieldCheck size={14} />}
            {busy ? '正在执行三组探针' : result ? '重新检测' : '开始验模'}
          </button>
        </>
      }
    >
      {item && (
        <div className="monitor-integrity-dialog">
          <div className="monitor-integrity-notice">
            <ShieldCheck size={16} />
            <span>
              将直连该中转站执行结构化输出、工具调用和多轮指令三组动态挑战。
              会产生少量 Token 消耗，检测数据独立保存。
            </span>
          </div>
          <div className="field">
            <label htmlFor="integrityModel">标称模型</label>
            <input
              id="integrityModel"
              value={model}
              list={modelListId}
              onChange={(event) => onModelChange(event.target.value)}
              placeholder="例如 gpt-5"
              autoComplete="off"
              disabled={busy}
            />
            <datalist id={modelListId}>
              {item.models
                .filter((candidate) => candidate !== '*' && !candidate.includes('*'))
                .map((candidate) => <option value={candidate} key={candidate} />)}
            </datalist>
          </div>

          {result && (
            <div className={`monitor-integrity-result ${result.risk}`}>
              <div className="monitor-integrity-result-head">
                <div>
                  <span>风险判断</span>
                  <strong>{integrityRiskLabel(result.risk)}</strong>
                </div>
                <b>{result.score}<small>/100</small></b>
              </div>
              <p>{result.summary}</p>
              <div className="monitor-integrity-facts">
                <span>有效探针 <strong>{result.successful_probes}/{result.probe_count}</strong></span>
                <span>Token <strong>{formatNumber(result.total_tokens)}</strong></span>
                <span>耗时 <strong>{formatLatency(result.duration_ms)}</strong></span>
                <span>响应模型 <strong>{result.observed_models.join('、') || '未提供'}</strong></span>
              </div>
              <div className="monitor-integrity-checks">
                {result.checks.map((check, index) => (
                  <div className={check.status} key={`${check.key}-${index}`}>
                    <span>{check.status === 'pass' ? '通过' : check.status === 'warn' ? '提示' : '异常'}</span>
                    <div>
                      <strong>{check.label}</strong>
                      <small>{check.message}</small>
                    </div>
                  </div>
                ))}
              </div>
              <small className="monitor-integrity-disclaimer">
                黑盒检测只能提供风险证据；中转站仍可能伪造模型字段或代理真实模型。
              </small>
            </div>
          )}
        </div>
      )}
    </Dialog>
  )
}

function MonitorDetailDialog({ item, onClose }: { item: ChannelMonitorItem | null; onClose: () => void }) {
  return (
    <Dialog
      open={Boolean(item)}
      title={item ? `${item.name} · 最近观测` : '最近观测'}
      onClose={onClose}
      footer={<button className="btn" onClick={onClose}>关闭</button>}
    >
      {item && (
        <div className="monitor-detail-list">
          {item.timeline.map((event) => (
            <div className="monitor-detail-row" key={event.id}>
              <span className={`monitor-detail-status ${event.status}`}>{statusLabel(event.status)}</span>
              <div className="monitor-detail-main">
                <strong>{event.model || event.endpoint_family || '上游请求'}</strong>
                <span>{event.message || `${event.source === 'probe' ? '手动检测' : '真实流量'}完成`}</span>
              </div>
              <div className="monitor-detail-values">
                <strong>首包 {formatLatency(event.ttfb_ms)}</strong>
                <span>总计 {formatLatency(event.duration_ms)}{event.http_status ? ` · HTTP ${event.http_status}` : ''}</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </Dialog>
  )
}

function integrityRiskLabel(risk: ModelIntegrityResult['risk']) {
  switch (risk) {
    case 'normal': return '暂未发现异常'
    case 'suspicious': return '可疑'
    case 'high_risk': return '高风险'
    default: return '无法判断'
  }
}

function displayStatus(item: ChannelMonitorItem): ChannelMonitorStatus | null {
  return item.account_status === 'disabled' ? null : item.latest_status
}

function statusLabel(status: ChannelMonitorStatus | null) {
  switch (status) {
    case 'operational': return '正常'
    case 'degraded': return '响应较慢'
    case 'failed': return '响应失败'
    case 'error': return '连接异常'
    default: return '暂无数据'
  }
}

function availabilityTone(value: number | null | undefined): 'healthy' | 'warning' | 'danger' | 'neutral' {
  if (value === null || value === undefined) return 'neutral'
  if (value < 90) return 'danger'
  if (value < 99) return 'warning'
  return 'healthy'
}

function capacityPercent(item: ChannelMonitorItem) {
  if (item.concurrency <= 0) return 0
  return Math.min(100, Math.max(0, item.current_capacity / item.concurrency * 100))
}

function formatPercent(value: number | null | undefined) {
  return value === null || value === undefined || !Number.isFinite(value)
    ? '--'
    : `${value.toFixed(value >= 99 ? 2 : 1)}%`
}

function formatLatency(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) return '--'
  if (value >= 1000) return `${(value / 1000).toFixed(value >= 10_000 ? 1 : 2)}s`
  return `${Math.round(value)}ms`
}

function formatNumber(value: number | null | undefined) {
  return Number(value ?? 0).toLocaleString('zh-CN')
}

function formatUsd(value: number) {
  if (!Number.isFinite(value)) return '$0.0000'
  return `$${value.toFixed(value < 0.01 ? 6 : 4)}`
}
