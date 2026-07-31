import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type { ProxyInfo } from '../types'

export function AppHeader({ proxy, onSecretAction }: { proxy: ProxyInfo | null; onSecretAction?: () => void }) {
  const serviceLabel = proxy?.running ? '代理运行中' : proxy ? '端口不可用' : '正在启动'
  const [maximized, setMaximized] = useState(false)
  const clickCount = useRef(0)
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const handleBrandClick = () => {
    clickCount.current += 1
    if (clickTimer.current) clearTimeout(clickTimer.current)
    if (clickCount.current >= 3) {
      clickCount.current = 0
      onSecretAction?.()
      return
    }
    clickTimer.current = setTimeout(() => { clickCount.current = 0 }, 400)
  }

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return
    const win = getCurrentWindow()
    win.isMaximized().then(setMaximized).catch(() => undefined)
    const unlisten = win.onResized(() => {
      win.isMaximized().then(setMaximized).catch(() => undefined)
    })
    return () => { unlisten.then((fn) => fn()).catch(() => undefined) }
  }, [])

  return (
    <header className="app-header" data-tauri-drag-region>
      <div className="brand" data-tauri-drag-region>
        <div className="brand-mark" aria-hidden="true" onClick={handleBrandClick}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none">
            <path d="M12 21V14" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round"/>
            <path d="M12 14L6 7" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"/>
            <path d="M12 14V5" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"/>
            <path d="M12 14L18 7" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round"/>
            <circle cx="12" cy="21" r="2" fill="currentColor"/>
            <circle cx="6" cy="7" r="1.6" fill="currentColor" opacity="0.8"/>
            <circle cx="12" cy="5" r="1.6" fill="currentColor" opacity="0.8"/>
            <circle cx="18" cy="7" r="1.6" fill="currentColor" opacity="0.8"/>
          </svg>
        </div>
        <div className="brand-copy" data-tauri-drag-region>
          <h1 className="brand-name">Aether</h1>
          <div className="brand-meta">个人 AI 上游网关</div>
        </div>
      </div>
      <div className="header-right">
        <div className="service-state" data-tauri-drag-region>
          <span className={`state-dot${proxy?.running ? ' online' : ''}`} />
          <span>{serviceLabel}</span>
        </div>
        <div className="win-controls">
          <button
            className="win-btn"
            onClick={() => getCurrentWindow().minimize()}
            data-tooltip="最小化"
            aria-label="最小化"
          >
            <svg width="10" height="1" viewBox="0 0 10 1"><rect width="10" height="1" fill="currentColor" /></svg>
          </button>
          <button
            className="win-btn"
            onClick={() => getCurrentWindow().toggleMaximize()}
            data-tooltip={maximized ? '还原' : '最大化'}
            aria-label={maximized ? '还原' : '最大化'}
          >
            {maximized ? (
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
                <rect x="2.5" y="0.5" width="7" height="7" rx="0.5" />
                <path d="M0.5 3.5V9.5H6.5" />
              </svg>
            ) : (
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
                <rect x="0.5" y="0.5" width="9" height="9" rx="0.5" />
              </svg>
            )}
          </button>
          <button
            className="win-btn win-btn-close"
            onClick={() => getCurrentWindow().close()}
            data-tooltip="关闭"
            aria-label="关闭"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
              <path d="M1 1L9 9M9 1L1 9" />
            </svg>
          </button>
        </div>
      </div>
    </header>
  )
}
