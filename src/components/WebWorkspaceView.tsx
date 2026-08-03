import { useLayoutEffect, useMemo, useRef } from 'react'
import { syncWebviewTabs } from '../lib/webviewTabs'
import type { WebWorkspaceTab, WorkspaceTab } from '../lib/workspaceTabs'

interface WebWorkspaceViewProps {
  activeTab: WebWorkspaceTab | null
  tabs: WorkspaceTab[]
  onError: (message: string) => void
}

export function WebWorkspaceView({
  activeTab,
  tabs,
  onError,
}: WebWorkspaceViewProps) {
  const hostRef = useRef<HTMLDivElement>(null)
  const lastErrorRef = useRef('')
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
          .then(() => { lastErrorRef.current = '' })
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
  }, [activeTab?.id, activeTab?.url, onError, openTabsKey])

  return activeTab ? (
    <div
      className="web-tab-host"
      data-webview-tab-id={activeTab.id}
      ref={hostRef}
    />
  ) : null
}
