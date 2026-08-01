import { useEffect, useRef, useState, type ComponentType, type SVGProps } from 'react'
import {
  Activity,
  Check,
  Globe2,
  EyeOff,
  Plus,
  ScrollText,
  Server,
  Settings,
  Store,
  Terminal,
  X,
} from 'lucide-react'
import {
  INTERNAL_PAGE_IDS,
  internalTabId,
  type InternalPageId,
  type TabDropEdge,
  type WorkspaceTab,
} from '../lib/workspaceTabs'

type TabIcon = ComponentType<SVGProps<SVGSVGElement> & { size?: number | string }>

interface WorkspaceTabBarProps {
  tabs: WorkspaceTab[]
  activeTabId: string
  badges?: Partial<Record<InternalPageId, number>>
  pickerOpen: boolean
  onPickerOpenChange: (open: boolean) => void
  onActivate: (tabId: string) => void
  onClose: (tabId: string) => void
  onHide: (tabId: string) => void
  onOpenPage: (page: InternalPageId) => void
  onMove: (sourceId: string, targetId: string, edge: TabDropEdge) => void
}

const PAGE_META: Record<InternalPageId, { label: string; icon: TabIcon }> = {
  upstreams: { label: '上游管理', icon: Server },
  codex: { label: 'Codex 配置', icon: Terminal },
  monitor: { label: '渠道监控', icon: Activity },
  market: { label: '市场监控', icon: Store },
  logs: { label: '请求日志', icon: ScrollText },
  settings: { label: '设置', icon: Settings },
}

interface DropTarget {
  id: string
  edge: TabDropEdge
}

export function WorkspaceTabBar({
  tabs,
  activeTabId,
  badges = {},
  pickerOpen,
  onPickerOpenChange,
  onActivate,
  onClose,
  onHide,
  onOpenPage,
  onMove,
}: WorkspaceTabBarProps) {
  const pickerRef = useRef<HTMLDivElement>(null)
  const addButtonRef = useRef<HTMLButtonElement>(null)
  const firstMenuItemRef = useRef<HTMLButtonElement>(null)
  const draggedTabIdRef = useRef<string | null>(null)
  const [dropTarget, setDropTarget] = useState<DropTarget | null>(null)

  useEffect(() => {
    if (!pickerOpen) return
    const onPointerDown = (event: PointerEvent) => {
      if (!pickerRef.current?.contains(event.target as Node)) onPickerOpenChange(false)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      onPickerOpenChange(false)
      addButtonRef.current?.focus()
    }
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown)
    const focusFrame = window.requestAnimationFrame(() => firstMenuItemRef.current?.focus())
    return () => {
      document.removeEventListener('pointerdown', onPointerDown)
      document.removeEventListener('keydown', onKeyDown)
      window.cancelAnimationFrame(focusFrame)
    }
  }, [onPickerOpenChange, pickerOpen])

  useEffect(() => {
    document.getElementById(`workspace-tab-${activeTabId}`)?.scrollIntoView({
      block: 'nearest',
      inline: 'nearest',
    })
  }, [activeTabId])

  const clearDragState = () => {
    draggedTabIdRef.current = null
    setDropTarget(null)
  }

  const focusTab = (tabId: string) => {
    onActivate(tabId)
    window.requestAnimationFrame(() => {
      document.getElementById(`workspace-tab-${tabId}`)?.focus()
    })
  }

  return (
    <nav className="workspace-tabs" aria-label="工作区标签页">
      <div
        className="workspace-tab-list"
        role="tablist"
        aria-label="已打开的页面"
        onWheel={(event) => {
          if (Math.abs(event.deltaY) > Math.abs(event.deltaX)) {
            event.currentTarget.scrollLeft += event.deltaY
          }
        }}
      >
        {tabs.map((tab) => {
          const meta = tabMeta(tab)
          const Icon = meta.icon
          const active = tab.id === activeTabId
          const badge = tab.kind === 'internal' ? badges[tab.page] ?? 0 : 0
          const target = dropTarget?.id === tab.id ? dropTarget.edge : null
          return (
            <div
              className={`workspace-tab${active ? ' active' : ''}${target ? ` drop-${target}` : ''}`}
              key={tab.id}
              draggable
              onDragStart={(event) => {
                event.dataTransfer.effectAllowed = 'move'
                event.dataTransfer.setData('text/plain', tab.id)
                draggedTabIdRef.current = tab.id
              }}
              onDragOver={(event) => {
                const sourceId = draggedTabIdRef.current
                if (!sourceId || sourceId === tab.id) return
                event.preventDefault()
                event.dataTransfer.dropEffect = 'move'
                const bounds = event.currentTarget.getBoundingClientRect()
                const nextTarget: DropTarget = {
                  id: tab.id,
                  edge: event.clientX < bounds.left + bounds.width / 2 ? 'before' : 'after',
                }
                setDropTarget(nextTarget)
              }}
              onDrop={(event) => {
                event.preventDefault()
                const sourceId = event.dataTransfer.getData('text/plain') || draggedTabIdRef.current
                const bounds = event.currentTarget.getBoundingClientRect()
                const edge = event.clientX < bounds.left + bounds.width / 2 ? 'before' : 'after'
                if (sourceId && sourceId !== tab.id) {
                  onMove(sourceId, tab.id, edge)
                }
                clearDragState()
              }}
              onDragEnd={clearDragState}
            >
              <button
                className="workspace-tab-main"
                id={`workspace-tab-${tab.id}`}
                type="button"
                role="tab"
                aria-selected={active}
                aria-controls="workspace-panel"
                tabIndex={active ? 0 : -1}
                onClick={() => onActivate(tab.id)}
                onAuxClick={(event) => {
                  if (event.button === 1) onClose(tab.id)
                }}
                onKeyDown={(event) => {
                  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
                  event.preventDefault()
                  const index = tabs.findIndex((item) => item.id === tab.id)
                  const nextIndex = event.key === 'Home'
                    ? 0
                    : event.key === 'End'
                      ? tabs.length - 1
                      : (index + (event.key === 'ArrowLeft' ? -1 : 1) + tabs.length) % tabs.length
                  focusTab(tabs[nextIndex].id)
                }}
              >
                <Icon size={14} aria-hidden="true" />
                <span className="workspace-tab-title">{meta.label}</span>
                {badge > 0 && (
                  <span className="workspace-tab-badge" aria-label={`${badge} 条未读提醒`}>
                    {badge > 99 ? '99+' : badge}
                  </span>
                )}
              </button>
              {(tab.kind === 'web'
                || tabs.filter((item) => item.kind === 'internal').length > 1) && (
                <button
                  className="workspace-tab-close"
                  type="button"
                  aria-label={`${tab.kind === 'internal' ? '隐藏' : '关闭'}${meta.label}`}
                  data-tooltip={tab.kind === 'internal' ? '隐藏标签页' : '关闭标签页'}
                  onClick={(event) => {
                    event.stopPropagation()
                    if (tab.kind === 'internal') onHide(tab.id)
                    else onClose(tab.id)
                  }}
                >
                  {tab.kind === 'internal'
                    ? <EyeOff size={13} aria-hidden="true" />
                    : <X size={13} aria-hidden="true" />}
                </button>
              )}
            </div>
          )
        })}
      </div>

      <div className="workspace-tab-picker" ref={pickerRef}>
        <button
          className="workspace-tab-add"
          ref={addButtonRef}
          type="button"
          aria-label="新建标签页"
          aria-haspopup="menu"
          aria-expanded={pickerOpen}
          data-tooltip="新建标签页"
          onClick={() => onPickerOpenChange(!pickerOpen)}
        >
          <Plus size={16} aria-hidden="true" />
        </button>
        {pickerOpen && (
          <div className="workspace-tab-menu" role="menu" aria-label="打开页面">
            {INTERNAL_PAGE_IDS.map((page, index) => {
              const meta = PAGE_META[page]
              const Icon = meta.icon
              const open = tabs.some((tab) => tab.id === internalTabId(page))
              return (
                <button
                  className="workspace-tab-menu-item"
                  ref={index === 0 ? firstMenuItemRef : undefined}
                  type="button"
                  role="menuitem"
                  key={page}
                  onClick={() => {
                    onOpenPage(page)
                    onPickerOpenChange(false)
                  }}
                >
                  <Icon size={15} aria-hidden="true" />
                  <span>{meta.label}</span>
                  {open && <Check className="workspace-tab-menu-check" size={14} aria-hidden="true" />}
                </button>
              )
            })}
          </div>
        )}
      </div>
    </nav>
  )
}

function tabMeta(tab: WorkspaceTab): { label: string; icon: TabIcon } {
  if (tab.kind === 'internal') return PAGE_META[tab.page]
  return { label: tab.title || webHost(tab.url), icon: Globe2 }
}

function webHost(value: string) {
  try {
    return new URL(value).hostname
  } catch {
    return value
  }
}
