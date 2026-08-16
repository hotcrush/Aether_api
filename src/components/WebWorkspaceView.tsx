import { RefreshCw } from 'lucide-react'
import { memo, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { syncWebviewTabs } from '../lib/webviewTabs'
import type { WebWorkspaceTab, WorkspaceTab } from '../lib/workspaceTabs'

interface WebWorkspaceViewProps {
  activeTab: WebWorkspaceTab | null
  tabs: WorkspaceTab[]
  onError: (message: string) => void
}

export const WebWorkspaceView = memo(function WebWorkspaceView({
  activeTab,
  tabs,
  onError,
}: WebWorkspaceViewProps) {
  const hostRef = useRef<HTMLDivElement>(null)
  const lastErrorRef = useRef('')
  const [syncError, setSyncError] = useState('')
  const [retryNonce, setRetryNonce] = useState(0)
  const openTabIds = useMemo(
    () => tabs.filter((tab) => tab.kind === 'web').map((tab) => tab.id),
    [tabs],
  )
  const openTabsKey = openTabIds.join('\u0000')

  useLayoutEffect(() => {
    let frame = 0
    const reportError = (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error)
      if (message === lastErrorRef.current) return
      lastErrorRef.current = message
      setSyncError(message)
      onError(message)
    }
    const sync = () => {
      window.cancelAnimationFrame(frame)
      frame = window.requestAnimationFrame(() => {
        const host = hostRef.current
        const bounds = host?.getBoundingClientRect()
        if (activeTab && (!bounds || bounds.width < 1 || bounds.height < 1)) return

        void syncWebviewTabs({
          active: activeTab ? {
            tabId: activeTab.id,
            url: activeTab.url,
            useOutboundProxy: activeTab.source?.kind === 'oauth',
          } : null,
          openTabIds,
          bounds: bounds ? {
            x: bounds.left,
            y: bounds.top,
            width: bounds.width,
            height: bounds.height,
          } : undefined,
        })
          .then(() => {
            lastErrorRef.current = ''
            setSyncError('')
          })
          .catch(reportError)
      })
    }

    sync()
    if (!activeTab || !hostRef.current) {
      return () => window.cancelAnimationFrame(frame)
    }

    const observer = new ResizeObserver(sync)
    observer.observe(hostRef.current)
    window.addEventListener('resize', sync)
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', sync)
      window.cancelAnimationFrame(frame)
    }
  }, [activeTab?.id, activeTab?.url, onError, openTabsKey, retryNonce])

  return activeTab ? (
    <div
      className="web-tab-host"
      data-webview-tab-id={activeTab.id}
      ref={hostRef}
    >
      {syncError && (
        <div className="web-tab-error" role="alert">
          <strong>内置页面无法打开</strong>
          <span>{syncError}</span>
          <button className="btn" type="button" onClick={() => setRetryNonce((value) => value + 1)}>
            <RefreshCw size={14} />重新加载
          </button>
        </div>
      )}
    </div>
  ) : null
})
