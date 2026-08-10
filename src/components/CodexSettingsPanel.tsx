import { useEffect, useMemo, useState } from 'react'
import {
  CircleAlert,
  CircleCheck,
  Copy,
  Database,
  Eye,
  EyeOff,
  FileText,
  History,
  Power,
  RotateCcw,
  ShieldCheck,
  Terminal,
} from 'lucide-react'
import type { CodexSessionHistoryStatus, CodexTakeoverStatus, ProxyInfo } from '../types'

export interface CodexSettingsPanelProps {
  proxy: ProxyInfo | null
  status: CodexTakeoverStatus | null
  sessionHistory: CodexSessionHistoryStatus | null
  busy: boolean
  migrateHistoryBusy: boolean
  restoreHistoryBusy: boolean
  resetTokenBusy: boolean
  onCopy: (value: string) => void
  onToggleTakeover: () => void
  onMigrateHistory: () => void
  onRestoreHistory: () => void
  onResetAccessToken: () => void
}

export function CodexSettingsPanel({
  proxy,
  status,
  sessionHistory,
  busy,
  migrateHistoryBusy,
  restoreHistoryBusy,
  resetTokenBusy,
  onCopy,
  onToggleTakeover,
  onMigrateHistory,
  onRestoreHistory,
  onResetAccessToken,
}: CodexSettingsPanelProps) {
  const [keyVisible, setKeyVisible] = useState(false)
  const baseUrl = proxy ? `${proxy.base_url}/v1` : status?.expected_base_url ?? ''
  const restorable = Boolean(status?.active || status?.backup_available)
  const statusKind = status?.active ? 'active' : status?.backup_available ? 'restore' : 'idle'
  const statusLabel = status?.active ? '已接管' : status?.backup_available ? '可恢复' : '未接管'
  const actionLabel = restorable ? '恢复配置' : '接管 Codex'
  const actionTitle = restorable
    ? '仅恢复接管前的 auth.json 与 config.toml；会话需在下方单独恢复'
    : '备份 auth.json 与 config.toml，再把 Codex Provider 指向 Aether 本地代理'
  const historyStatusKind = sessionHistory?.active ? 'active' : 'idle'
  const historyStatusLabel = sessionHistory?.active ? '已统一' : '待接管'
  const historyBusy = migrateHistoryBusy || restoreHistoryBusy
  const migrateDisabled = !sessionHistory?.active || historyBusy
  const restoreDisabled = !sessionHistory?.backup_available || historyBusy
  const statePathsText = sessionHistory?.state_paths.length
    ? sessionHistory.state_paths.join(' ; ')
    : sessionHistory ? '未发现 state_5.sqlite' : '加载中'

  const configRows = useMemo(() => [
    ['Provider', status?.provider_id ? `${status.provider_id} / Aether Local` : 'custom'],
    ['模型', status?.model || 'gpt-5.5'],
    ['当前指向', status?.configured_base_url || '未指向 Aether'],
    ['目标地址', status?.expected_base_url || baseUrl || '加载中'],
    ['配置文件', status?.config_path || '加载中'],
    ['认证文件', status?.auth_path || '加载中'],
  ], [baseUrl, status])

  const historyRows = useMemo(() => [
    ['Provider 桶', sessionHistory?.provider_id || 'custom'],
    ['会话目录', sessionHistory?.sessions_path || '加载中'],
    ['归档目录', sessionHistory?.archived_sessions_path || '加载中'],
    ['State DB', statePathsText],
  ], [sessionHistory, statePathsText])

  useEffect(() => {
    setKeyVisible(false)
  }, [proxy?.access_token])

  return (
    <main className="codex-page">
      <section className="codex-config" aria-label="Codex 配置">
        <div className="codex-config-head">
          <div className="codex-heading">
            <div className="codex-title">
              <Terminal size={18} />
              Codex 配置
            </div>
            <div className="codex-subtitle">
              配置接管与既有会话迁移分开处理
            </div>
          </div>
          <div className="codex-head-actions">
            <div className="codex-action-meta">
              <ShieldCheck size={15} />
              {status?.backup_available
                ? '可恢复接管前的 auth.json 与 config.toml'
                : '接管前备份 auth.json 与 config.toml'}
            </div>
            <span className={`codex-status ${statusKind}`}>
              {status?.active ? <CircleCheck size={14} /> : <CircleAlert size={14} />}
              {statusLabel}
            </span>
            <button
              className={`btn ${restorable ? '' : 'btn-primary'}`}
              onClick={onToggleTakeover}
              disabled={!proxy || busy}
              data-tooltip={actionTitle}
            >
              {restorable ? <RotateCcw className={busy ? 'spin' : undefined} size={16} /> : <Power size={16} />}
              {actionLabel}
            </button>
          </div>
        </div>

        <div className="codex-access-grid">
          <div className="codex-access-block">
            <span className="info-label">BASE URL</span>
            <div className="copy-field">
              <code>{baseUrl || '加载中'}</code>
              <button
                className="icon-btn"
                onClick={() => onCopy(baseUrl)}
                disabled={!baseUrl}
                data-tooltip="复制 Base URL"
                aria-label="复制 Base URL"
              >
                <Copy size={15} />
              </button>
            </div>
          </div>
          <div className="codex-access-block">
            <span className="info-label">API KEY</span>
            <div className="copy-field">
              <code>{proxy ? (keyVisible ? proxy.access_token : maskSecret(proxy.access_token)) : '加载中'}</code>
              <button
                className="icon-btn"
                onClick={() => setKeyVisible((visible) => !visible)}
                disabled={!proxy}
                data-tooltip={keyVisible ? '隐藏 API Key' : '显示 API Key'}
                aria-label={keyVisible ? '隐藏 API Key' : '显示 API Key'}
              >
                {keyVisible ? <EyeOff size={15} /> : <Eye size={15} />}
              </button>
              <button
                className="icon-btn"
                onClick={() => proxy && onCopy(proxy.access_token)}
                disabled={!proxy}
                data-tooltip="复制 API Key"
                aria-label="复制 API Key"
              >
                <Copy size={15} />
              </button>
              <button
                className="icon-btn key-reset"
                onClick={onResetAccessToken}
                disabled={!proxy || resetTokenBusy}
                data-tooltip="重置 API Key"
                aria-label="重置 API Key"
              >
                <RotateCcw className={resetTokenBusy ? 'spin' : undefined} size={15} />
              </button>
            </div>
          </div>
        </div>

        <div className="codex-detail-list">
          {configRows.map(([label, value]) => (
            <div className="codex-detail-row" key={label}>
              <span>{label}</span>
              <code>{value}</code>
              {(label === '配置文件' || label === '认证文件' || label === '目标地址' || label === '当前指向') && (
                <button
                  className="icon-btn"
                  onClick={() => onCopy(value)}
                  disabled={!value || value === '加载中' || value === '未指向 Aether'}
                  data-tooltip={`复制${label}`}
                  aria-label={`复制${label}`}
                >
                  {label.endsWith('文件') ? <FileText size={15} /> : <Copy size={15} />}
                </button>
              )}
            </div>
          ))}
        </div>

        <div className="codex-history-section" aria-label="Codex 会话统一">
          <div className="codex-history-head">
            <div className="codex-heading">
              <div className="codex-title">
                <History size={18} />
                会话统一
              </div>
              <div className="codex-subtitle">
                新会话进入 {sessionHistory?.provider_id || 'custom'}；既有会话需单独迁移
              </div>
            </div>
            <div className="codex-head-actions">
              <div className="codex-action-meta">
                <ShieldCheck size={15} />
                {sessionHistory?.backup_available ? '可按账本恢复原 openai 会话' : '迁移前生成会话备份账本'}
              </div>
              <span className={`codex-status ${historyStatusKind}`}>
                {sessionHistory?.active ? <CircleCheck size={14} /> : <CircleAlert size={14} />}
                {historyStatusLabel}
              </span>
              <button
                className="btn"
                onClick={onMigrateHistory}
                disabled={migrateDisabled}
                data-tooltip="将 openai/aether 会话的 Provider 标记迁移到 custom"
              >
                <Database className={migrateHistoryBusy ? 'spin' : undefined} size={16} />
                迁移既有会话
              </button>
              <button
                className="btn"
                onClick={onRestoreHistory}
                disabled={restoreDisabled}
                data-tooltip="仅恢复账本中原属 openai 的会话，不恢复配置或其他 custom 会话"
              >
                <RotateCcw className={restoreHistoryBusy ? 'spin' : undefined} size={16} />
                恢复官方会话
              </button>
            </div>
          </div>

          <div className="codex-detail-list codex-history-list">
            {historyRows.map(([label, value]) => {
              const canCopy = Boolean(value && value !== '加载中' && !value.startsWith('未发现'))
              return (
                <div className="codex-detail-row" key={label}>
                  <span>{label}</span>
                  <code>{value}</code>
                  <button
                    className="icon-btn"
                    onClick={() => onCopy(value)}
                    disabled={!canCopy}
                    data-tooltip={`复制${label}`}
                    aria-label={`复制${label}`}
                  >
                    {label === 'Provider 桶' ? <Copy size={15} /> : <FileText size={15} />}
                  </button>
                </div>
              )
            })}
          </div>
        </div>
      </section>
    </main>
  )
}

function maskSecret(secret: string) {
  if (!secret) return ''
  if (secret.length <= 13) return '••••••••••••'
  return `${secret.slice(0, 9)}••••••••••••${secret.slice(-4)}`
}
