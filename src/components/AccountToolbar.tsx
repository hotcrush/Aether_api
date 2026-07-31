import {
  ChevronDown,
  Download,
  Gauge,
  MoreHorizontal,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  Upload,
} from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import type { AccountStatus, AccountTypeFilter } from '../types'

const AUTO_REFRESH_INTERVALS = [5, 15, 30, 60] as const

interface AccountToolbarProps {
  query: string
  statusFilter: 'all' | AccountStatus
  typeFilter: AccountTypeFilter
  autoRefreshEnabled: boolean
  autoRefreshIntervalMinutes: number
  refreshAllBusy: boolean
  queryAllBusy: boolean
  removeErrorsBusy: boolean
  errorCount: number
  onQueryChange: (value: string) => void
  onStatusFilterChange: (value: 'all' | AccountStatus) => void
  onTypeFilterChange: (value: AccountTypeFilter) => void
  onAutoRefreshEnabledChange: (enabled: boolean) => void
  onAutoRefreshIntervalChange: (minutes: number) => void
  onImport: () => void
  onRefreshAll: () => void
  onQueryAll: () => void
  onExport: () => void
  onAddRelay: () => void
  onRemoveErrors: () => void
}

export function AccountToolbar({
  query,
  statusFilter,
  typeFilter,
  autoRefreshEnabled,
  autoRefreshIntervalMinutes,
  refreshAllBusy,
  queryAllBusy,
  removeErrorsBusy,
  errorCount,
  onQueryChange,
  onStatusFilterChange,
  onTypeFilterChange,
  onAutoRefreshEnabledChange,
  onAutoRefreshIntervalChange,
  onImport,
  onRefreshAll,
  onQueryAll,
  onExport,
  onAddRelay,
  onRemoveErrors,
}: AccountToolbarProps) {
  const [usageMenuOpen, setUsageMenuOpen] = useState(false)
  const [moreOpen, setMoreOpen] = useState(false)
  const usageMenuRef = useRef<HTMLDivElement>(null)
  const usageMenuButtonRef = useRef<HTMLButtonElement>(null)
  const autoRefreshToggleRef = useRef<HTMLButtonElement>(null)
  const moreRef = useRef<HTMLDivElement>(null)
  const moreButtonRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    const closeMenus = (event: MouseEvent) => {
      if (!usageMenuRef.current?.contains(event.target as Node)) setUsageMenuOpen(false)
      if (!moreRef.current?.contains(event.target as Node)) setMoreOpen(false)
    }

    const closeMenusOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return

      if (usageMenuOpen) {
        setUsageMenuOpen(false)
        usageMenuButtonRef.current?.focus()
      } else if (moreOpen) {
        setMoreOpen(false)
        moreButtonRef.current?.focus()
      }
    }

    document.addEventListener('mousedown', closeMenus)
    document.addEventListener('keydown', closeMenusOnEscape)
    return () => {
      document.removeEventListener('mousedown', closeMenus)
      document.removeEventListener('keydown', closeMenusOnEscape)
    }
  }, [moreOpen, usageMenuOpen])

  useEffect(() => {
    if (usageMenuOpen) autoRefreshToggleRef.current?.focus()
  }, [usageMenuOpen])

  const toggleUsageMenu = () => {
    setMoreOpen(false)
    setUsageMenuOpen((current) => !current)
  }

  const toggleMoreMenu = () => {
    setUsageMenuOpen(false)
    setMoreOpen((current) => !current)
  }

  const runMenuAction = (action: () => void) => {
    setMoreOpen(false)
    action()
  }

  return (
    <div className="toolbar">
      <div className="search-box">
        <Search className="search-icon" size={16} />
        <input
          type="search"
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder="搜索上游"
          aria-label="搜索上游"
        />
      </div>
      <div className="toolbar-filters">
        <div className="segmented" aria-label="上游类型筛选">
          {([['all', '全部'], ['oauth', '账号池'], ['api_key', '中转站']] as const).map(
            ([value, label]) => (
              <button
                key={value}
                className={`segment${typeFilter === value ? ' active' : ''}`}
                onClick={() => onTypeFilterChange(value)}
                aria-pressed={typeFilter === value}
              >
                {label}
              </button>
            ),
          )}
        </div>
        <div className="segmented" aria-label="上游状态筛选">
          {([['all', '全部'], ['active', '启用'], ['disabled', '停用']] as const).map(
            ([value, label]) => (
              <button
                key={value}
                className={`segment${statusFilter === value ? ' active' : ''}`}
                onClick={() => onStatusFilterChange(value)}
                aria-pressed={statusFilter === value}
              >
                {label}
              </button>
            ),
          )}
        </div>
      </div>
      <div className="toolbar-spacer" />
      <div className="menu-wrap usage-query-split" ref={usageMenuRef}>
        <button
          type="button"
          className="btn usage-query-main"
          onClick={onQueryAll}
          disabled={queryAllBusy}
          data-tooltip="查询全部用量"
        >
          <Gauge size={16} />查询全部用量
        </button>
        <button
          ref={usageMenuButtonRef}
          type="button"
          className={`btn usage-query-toggle${autoRefreshEnabled ? ' active' : ''}`}
          onClick={toggleUsageMenu}
          aria-label="用量自动刷新设置"
          aria-haspopup="dialog"
          aria-expanded={usageMenuOpen}
          aria-controls="usage-auto-refresh-menu"
          data-tooltip="自动刷新设置"
        >
          <ChevronDown size={14} />
        </button>
        {usageMenuOpen && (
          <div
            id="usage-auto-refresh-menu"
            className="dropdown usage-query-dropdown"
            role="dialog"
            aria-label="用量自动刷新设置"
          >
            <button
              ref={autoRefreshToggleRef}
              type="button"
              className="auto-refresh-toggle"
              role="switch"
              aria-checked={autoRefreshEnabled}
              onClick={() => onAutoRefreshEnabledChange(!autoRefreshEnabled)}
            >
              <span className="auto-refresh-copy">
                <strong>自动刷新</strong>
                <small>
                  {autoRefreshEnabled ? `每 ${autoRefreshIntervalMinutes} 分钟` : '已关闭'}
                </small>
              </span>
              <span className="auto-refresh-switch" aria-hidden="true">
                <span />
              </span>
            </button>
            <div className="menu-separator" />
            <label className="auto-refresh-interval" htmlFor="usage-auto-refresh-interval">
              <span>刷新周期</span>
              <select
                id="usage-auto-refresh-interval"
                value={autoRefreshIntervalMinutes}
                onChange={(event) => onAutoRefreshIntervalChange(Number(event.target.value))}
              >
                {AUTO_REFRESH_INTERVALS.map((minutes) => (
                  <option key={minutes} value={minutes}>
                    {minutes} 分钟
                  </option>
                ))}
              </select>
            </label>
          </div>
        )}
      </div>
      <div className="menu-wrap" ref={moreRef}>
        <button
          ref={moreButtonRef}
          className="btn"
          onClick={toggleMoreMenu}
          aria-haspopup="menu"
          aria-expanded={moreOpen}
          data-tooltip="更多操作"
          aria-label="更多操作"
        >
          <MoreHorizontal size={16} />更多操作<ChevronDown size={14} />
        </button>
        {moreOpen && (
          <div className="dropdown">
            <button className="menu-item" onClick={() => runMenuAction(onImport)}>
              <Upload size={16} />导入上游
            </button>
            <button
              className="menu-item"
              onClick={() => runMenuAction(onRefreshAll)}
              disabled={refreshAllBusy}
            >
              <RefreshCw size={16} />刷新全部 OAuth
            </button>
            <div className="menu-separator" />
            <button className="menu-item" onClick={() => runMenuAction(onExport)}>
              <Download size={16} />导出备份
            </button>
            <button
              className="menu-item menu-item-danger"
              onClick={() => runMenuAction(onRemoveErrors)}
              disabled={removeErrorsBusy || errorCount === 0}
            >
              <Trash2 size={16} />移除报错上游{errorCount > 0 ? ` (${errorCount})` : ''}
            </button>
          </div>
        )}
      </div>
      <button className="btn btn-primary" onClick={onAddRelay} data-tooltip="添加中转站">
        <Plus size={16} />添加中转站
      </button>
    </div>
  )
}
