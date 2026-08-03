import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { ClipboardImportCandidate } from '../types'

export interface WebviewBounds {
  x: number
  y: number
  width: number
  height: number
}

export interface ActiveWebviewTab {
  tabId: string
  url: string
  useOutboundProxy?: boolean
}

export interface SyncWebviewTabsInput {
  active: ActiveWebviewTab | null
  openTabIds: string[]
  bounds?: WebviewBounds
}

export interface WebviewActivity {
  id: string
  tabId: string
  kind: 'copy' | 'download'
  phase: 'occurred' | 'requested' | 'finished'
  origin?: string
  fileName?: string
  success?: boolean
  occurredAt: string
}

export interface WebviewOpenRequested {
  id: string
  sourceTabId: string
  url: string
  title?: string
  occurredAt: string
}

let syncQueue: Promise<void> = Promise.resolve()

export function syncWebviewTabs(input: SyncWebviewTabsInput): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve()

  const next = syncQueue
    .catch(() => undefined)
    .then(() => invoke<void>('sync_webview_tabs', {
      active: input.active,
      openTabIds: input.openTabIds,
      bounds: input.bounds,
    }))
  syncQueue = next
  return next
}

export function listenWebviewActivity(
  handler: (activity: WebviewActivity) => void,
): Promise<UnlistenFn> {
  return listen<WebviewActivity>('webview:activity', ({ payload }) => handler(payload))
}

export function listenWebviewImportCandidate(
  handler: (candidate: ClipboardImportCandidate) => void,
): Promise<UnlistenFn> {
  return listen<ClipboardImportCandidate>(
    'webview:import-candidate',
    ({ payload }) => handler(payload),
  )
}

export function listenWebviewOpenRequested(
  handler: (request: WebviewOpenRequested) => void,
): Promise<UnlistenFn> {
  return listen<WebviewOpenRequested>(
    'webview:open-requested',
    ({ payload }) => handler(payload),
  )
}

function isTauriRuntime() {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}
