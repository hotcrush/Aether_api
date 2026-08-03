import { Activity, AlertTriangle, ChevronLeft, ChevronRight, ExternalLink, KeyRound, Pencil, RefreshCw, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { formatExpiry } from '../lib/time'
import type { Account, QuotaQueryState, RelayUsageQueryState } from '../types'
import { CapacityEditor } from './CapacityEditor'
import { PriorityEditor } from './PriorityEditor'
import { RateMultiplierEditor } from './RateMultiplierEditor'
import { QuotaPanel } from './QuotaPanel'
import { RelayUsagePanel } from './RelayUsagePanel'

const PAGE_SIZE = 10

const ERROR_CODE_TIPS: Record<string, string> = {
  '403': '403 Forbidden\n· Refresh token 已失效（长时间未使用被吊销）\n· 上游账号被封禁或暂停\n· 会话已失效，需要重新登录\n· 临时上游凭据已被回收',
  '401': '401 Unauthorized\n· Access token 过期且刷新失败\n· 上游凭据格式错误或已被撤销\n· 上游账号密码已变更',
  '429': '429 Too Many Requests\n· 请求频率超出限制\n· 额度窗口已用尽，等待重置\n· 单个上游并发过高',
  '500': '5xx 服务端错误\n· 上游服务暂时不可用\n· 稍后重试通常可恢复',
  '502': '5xx 服务端错误\n· 上游网关超时\n· 稍后重试通常可恢复',
  '503': '5xx 服务端错误\n· 上游服务过载或维护中\n· 稍后重试通常可恢复',
}

function getErrorTip(errorMessage: string): string | null {
  if (!errorMessage) return null
  const match = errorMessage.match(/\b(40[13]|429|5\d{2})\b/)
  if (match) return ERROR_CODE_TIPS[match[1]] ?? null
  if (/refresh|刷新/i.test(errorMessage)) return ERROR_CODE_TIPS['403']
  return null
}

interface ParsedError {
  code: string
  label: string
  resetsAt: number | null
}

function parseError(errorMessage: string): ParsedError {
  const codeMatch = errorMessage.match(/\b(40[13]|429|5\d{2})\b/)
  const code = codeMatch?.[1] || ''

  let label = '异常'
  if (code === '429') label = '限流中'
  else if (code === '401' || code === '403' || /refresh|刷新/i.test(errorMessage)) label = '认证失效'
  else if (code.startsWith('5')) label = '服务异常'

  const effectiveCode = code || (/refresh|刷新/i.test(errorMessage) ? '403' : '')

  const resetsMatch = errorMessage.match(/"resets_at"\s*:\s*(\d{10,13})/)
  let resetsAt: number | null = null
  if (resetsMatch) {
    const raw = Number(resetsMatch[1])
    resetsAt = raw > 1e12 ? raw : raw * 1000
  }

  return { code: effectiveCode, label, resetsAt }
}

function formatCountdown(resetsAt: number): string {
  const diff = resetsAt - Date.now()
  if (diff <= 0) return '已恢复'
  const h = Math.floor(diff / 3600000)
  const m = Math.floor((diff % 3600000) / 60000)
  if (h > 0) return `${h}h ${m}m 自动恢复`
  if (m > 0) return `${m}m 自动恢复`
  return '< 1m 自动恢复'
}

interface AccountTableProps {
  accounts: Account[]
  hasAccounts: boolean
  loading: boolean
  loadError: string
  busyActions: Set<string>
  quotaStates: Record<string, QuotaQueryState>
  relayUsageStates: Record<string, RelayUsageQueryState>
  accountCapacities: Record<string, number>
  onRetry: () => void
  onToggle: (account: Account) => void
  onTest: (account: Account) => void
  onRefresh: (account: Account) => void
  onEdit: (account: Account) => void
  onOpenRelay: (account: Account) => void
  onQuota: (account: Account) => void
  onRelayUsage: (account: Account) => void
  onPriority: (account: Account, priority: number) => void
  onConcurrency: (account: Account, concurrency: number) => void
  onRateMultiplier: (account: Account, multiplier: number) => void
  onAutoSyncRateMultiplier: (account: Account, enabled: boolean) => void
  onSyncRateMultiplier: (account: Account) => void
  onDelete: (account: Account) => void
}

export function AccountTable({
  accounts,
  hasAccounts,
  loading,
  loadError,
  busyActions,
  quotaStates,
  relayUsageStates,
  accountCapacities,
  onRetry,
  onToggle,
  onTest,
  onRefresh,
  onEdit,
  onOpenRelay,
  onQuota,
  onRelayUsage,
  onPriority,
  onConcurrency,
  onRateMultiplier,
  onAutoSyncRateMultiplier,
  onSyncRateMultiplier,
  onDelete,
}: AccountTableProps) {
  const [page, setPage] = useState(0)
  const totalPages = Math.max(1, Math.ceil(accounts.length / PAGE_SIZE))
  const currentPage = Math.min(page, totalPages - 1)
  const pageAccounts = accounts.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE)

  return (
    <section className="table-shell" aria-label="上游列表">
      {accounts.length ? (
        <>
        <table>
          <thead>
            <tr>
              <th className="col-account">上游</th>
              <th className="col-type">类型</th>
              <th className="col-credential">凭据</th>
              <th className="col-usage">额度 / 路由</th>
              <th className="col-capacity">容量</th>
              <th className="col-priority">优先级</th>
              <th className="col-rate-multiplier">成本倍率</th>
              <th className="col-status">状态</th>
              <th className="col-actions" />
            </tr>
          </thead>
          <tbody>
            {pageAccounts.map((account) => (
              <AccountRow
                key={account.id}
                account={account}
                busyActions={busyActions}
                quotaState={quotaStates[account.id]}
                relayUsageState={relayUsageStates[account.id]}
                currentConcurrency={accountCapacities[account.id] ?? 0}
                onToggle={onToggle}
                onTest={onTest}
                onRefresh={onRefresh}
                onEdit={onEdit}
                onOpenRelay={onOpenRelay}
                onQuota={onQuota}
                onRelayUsage={onRelayUsage}
                onPriority={onPriority}
                onConcurrency={onConcurrency}
                onRateMultiplier={onRateMultiplier}
                onAutoSyncRateMultiplier={onAutoSyncRateMultiplier}
                onSyncRateMultiplier={onSyncRateMultiplier}
                onDelete={onDelete}
              />
            ))}
          </tbody>
        </table>
        {totalPages > 1 && (
          <div className="table-pagination">
            <button
              className="page-btn"
              disabled={currentPage === 0}
              onClick={() => setPage(currentPage - 1)}
              aria-label="上一页"
            >
              <ChevronLeft size={14} />
            </button>
            <span className="page-info">{currentPage + 1} / {totalPages}</span>
            <button
              className="page-btn"
              disabled={currentPage >= totalPages - 1}
              onClick={() => setPage(currentPage + 1)}
              aria-label="下一页"
            >
              <ChevronRight size={14} />
            </button>
          </div>
        )}
        </>
      ) : (
        <div className="empty-state">
          <div className="empty-icon">
            {loading ? <RefreshCw className="spin" size={19} /> : <KeyRound size={19} />}
          </div>
          <div>
            {loading
              ? '正在读取上游'
              : loadError
                ? '上游加载失败'
                : hasAccounts
                  ? '没有匹配的上游'
                  : '暂无上游'}
          </div>
          {loadError && !loading && (
            <button className="btn" onClick={onRetry}>
              <RefreshCw size={15} />重试
            </button>
          )}
        </div>
      )}
    </section>
  )
}

interface AccountRowProps {
  account: Account
  busyActions: Set<string>
  quotaState?: QuotaQueryState
  relayUsageState?: RelayUsageQueryState
  currentConcurrency: number
  onToggle: (account: Account) => void
  onTest: (account: Account) => void
  onRefresh: (account: Account) => void
  onEdit: (account: Account) => void
  onOpenRelay: (account: Account) => void
  onQuota: (account: Account) => void
  onRelayUsage: (account: Account) => void
  onPriority: (account: Account, priority: number) => void
  onConcurrency: (account: Account, concurrency: number) => void
  onRateMultiplier: (account: Account, multiplier: number) => void
  onAutoSyncRateMultiplier: (account: Account, enabled: boolean) => void
  onSyncRateMultiplier: (account: Account) => void
  onDelete: (account: Account) => void
}

function AccountRow({
  account,
  busyActions,
  quotaState,
  relayUsageState,
  currentConcurrency,
  onToggle,
  onTest,
  onRefresh,
  onEdit,
  onOpenRelay,
  onQuota,
  onRelayUsage,
  onPriority,
  onConcurrency,
  onRateMultiplier,
  onAutoSyncRateMultiplier,
  onSyncRateMultiplier,
  onDelete,
}: AccountRowProps) {
  const oauth = account.account_type === 'oauth'
  const relayBaseUrl = account.base_url?.trim() || 'https://api.openai.com'
  const detail = account.last_error
    || (oauth
      ? account.email || account.chatgpt_account_id || 'OAuth 上游'
      : relayBaseUrl)
  const expiry = formatExpiry(account.expires_at)
  const toggleBusy = busyActions.has(`toggle:${account.id}`)
  const testBusy = busyActions.has(`test:${account.id}`)
  const refreshBusy = busyActions.has(`refresh:${account.id}`)
  const openRelayBusy = busyActions.has(`open-relay:${account.id}`)
  const priorityBusy = busyActions.has(`priority:${account.id}`)
  const capacityBusy = busyActions.has(`concurrency:${account.id}`)
  const rateMultiplierBusy = busyActions.has(`rate-multiplier:${account.id}`)
  const hasRelayBaseUrl = Boolean(account.base_url?.trim())
  const tipContent = account.last_error ? (getErrorTip(account.last_error) || account.last_error) : null
  const parsedErr = account.last_error ? parseError(account.last_error) : null

  return (
    <tr>
      <td className="col-account">
        <div className="account-name" data-tooltip={account.name || undefined}>{account.name || '未命名'}</div>
        {parsedErr ? (
          <div
            className="account-detail account-error error-tip-wrap error-structured"
            data-tooltip={tipContent ?? undefined}
          >
            <div className="error-badges">
              <span className={`error-label error-label-${parsedErr.code === '429' ? 'rate' : parsedErr.code.startsWith('5') ? 'server' : parsedErr.code ? 'auth' : 'generic'}`}>{parsedErr.label}</span>
              {parsedErr.code && <span className="error-code-badge"><AlertTriangle size={11} />{parsedErr.code}</span>}
            </div>
            {parsedErr.resetsAt && <span className="error-countdown">{formatCountdown(parsedErr.resetsAt)}</span>}
          </div>
        ) : (
          <div className="account-detail" data-tooltip={detail}>{detail}</div>
        )}
      </td>
      <td className="col-type">
        <span className={`badge ${oauth ? 'badge-oauth' : 'badge-key'}`}>
          {oauth ? 'OAuth' : '中转站'}
        </span>
        {account.plan_type && <span className="badge badge-plan">{account.plan_type}</span>}
      </td>
      <td className="col-credential">
        <div className="credential">{account.credential_masked}</div>
        <div className="usage-label">
          {expiry || (oauth && account.refreshable ? '可自动续期' : '')}
        </div>
      </td>
      <td className="col-usage">
        {oauth ? (
          <QuotaPanel
            state={quotaState}
            requestCount={Number(account.request_count || 0)}
            lastUsedAt={account.last_used_at}
            onQuery={() => onQuota(account)}
          />
        ) : (
          <RelayUsagePanel
            state={relayUsageState}
            requestCount={Number(account.request_count || 0)}
            lastUsedAt={account.last_used_at}
            onQuery={() => onRelayUsage(account)}
          />
        )}
      </td>
      <td className="col-capacity">
        <CapacityEditor
          accountName={account.name}
          current={currentConcurrency}
          limit={account.concurrency ?? 10}
          busy={capacityBusy}
          onSave={(concurrency) => onConcurrency(account, concurrency)}
        />
      </td>
      <td className="col-priority">
        <PriorityEditor
          accountName={account.name}
          priority={account.priority ?? 1}
          busy={priorityBusy}
          onSave={(priority) => onPriority(account, priority)}
        />
      </td>
      <td className="col-rate-multiplier">
        <RateMultiplierEditor
          account={account}
          busy={rateMultiplierBusy}
          onSave={(multiplier) => onRateMultiplier(account, multiplier)}
          onAutoSync={(enabled) => onAutoSyncRateMultiplier(account, enabled)}
          onSync={() => onSyncRateMultiplier(account)}
        />
      </td>
      <td className="col-status">
        <button
          className={`status-button ${account.status}`}
          onClick={() => onToggle(account)}
          disabled={toggleBusy}
          role="switch"
          aria-checked={account.status === 'active'}
          aria-busy={toggleBusy}
          aria-label={`${account.name || '未命名'}启用状态`}
          data-tooltip={account.status === 'active' ? '点击停用' : '点击启用'}
        >
          <span className="status-switch-track" aria-hidden="true">
            <span className="status-switch-thumb" />
          </span>
          <span className="status-switch-label">
            {account.status === 'active' ? '启用' : '停用'}
          </span>
        </button>
      </td>
      <td className="col-actions">
        <div className="row-actions">
          <button
            className="icon-btn"
            onClick={() => onTest(account)}
            disabled={testBusy}
            data-tooltip="测试连接"
            aria-label="测试连接"
          >
            {testBusy ? <RefreshCw className="spin" size={16} /> : <Activity size={16} />}
          </button>
          {oauth && (
            <button
              className="icon-btn"
              onClick={() => onRefresh(account)}
              disabled={!account.refreshable || refreshBusy}
              data-tooltip="刷新 OAuth"
              aria-label="刷新 OAuth"
            >
              <RefreshCw className={refreshBusy ? 'spin' : undefined} size={16} />
            </button>
          )}
          {!oauth && (
            <button
              className="icon-btn"
              onClick={() => onOpenRelay(account)}
              disabled={!hasRelayBaseUrl || openRelayBusy}
              data-tooltip={hasRelayBaseUrl ? '打开中转站网页' : '未配置 Base URL'}
              aria-label={hasRelayBaseUrl ? '打开中转站网页' : '未配置 Base URL'}
              aria-busy={openRelayBusy}
            >
              {openRelayBusy
                ? <RefreshCw className="spin" size={16} />
                : <ExternalLink size={16} />}
            </button>
          )}
          <button
            className="icon-btn"
            onClick={() => onEdit(account)}
            data-tooltip="编辑账号"
            aria-label="编辑账号"
          >
            <Pencil size={16} />
          </button>
          <button
            className="icon-btn"
            onClick={() => onDelete(account)}
            data-tooltip="删除上游"
            aria-label="删除上游"
          >
            <Trash2 size={16} />
          </button>
        </div>
      </td>
    </tr>
  )
}
