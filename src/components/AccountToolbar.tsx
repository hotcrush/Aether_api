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

interface AccountToolbarProps {
  query: string
  statusFilter: 'all' | AccountStatus
  typeFilter: AccountTypeFilter
  reloadBusy: boolean
  refreshAllBusy: boolean
  queryAllBusy: boolean
  removeErrorsBusy: boolean
  errorCount: number
  onQueryChange: (value: string) => void
  onStatusFilterChange: (value: 'all' | AccountStatus) => void
  onTypeFilterChange: (value: AccountTypeFilter) => void
  onReload: () => void
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
  reloadBusy,
  refreshAllBusy,
  queryAllBusy,
  removeErrorsBusy,
  errorCount,
  onQueryChange,
  onStatusFilterChange,
  onTypeFilterChange,
  onReload,
  onImport,
  onRefreshAll,
  onQueryAll,
  onExport,
  onAddRelay,
  onRemoveErrors,
}: AccountToolbarProps) {
  const [moreOpen, setMoreOpen] = useState(false)
  const moreRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const closeMenu = (event: MouseEvent) => {
      if (!moreRef.current?.contains(event.target as Node)) setMoreOpen(false)
    }
    document.addEventListener('mousedown', closeMenu)
    return () => document.removeEventListener('mousedown', closeMenu)
  }, [])

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
      <button className="btn" onClick={onReload} disabled={reloadBusy} title="刷新列表">
        <RefreshCw className={reloadBusy ? 'spin' : undefined} size={16} />刷新
      </button>
      <div className="menu-wrap" ref={moreRef}>
        <button
          className="btn"
          onClick={() => setMoreOpen((current) => !current)}
          aria-expanded={moreOpen}
          title="更多操作"
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
            <button
              className="menu-item"
              onClick={() => runMenuAction(onQueryAll)}
              disabled={queryAllBusy}
            >
              <Gauge size={16} />查询全部用量
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
      <button className="btn btn-primary" onClick={onAddRelay} title="添加中转站">
        <Plus size={16} />添加中转站
      </button>
    </div>
  )
}
