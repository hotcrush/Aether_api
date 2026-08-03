export const INTERNAL_PAGE_IDS = [
  'upstreams',
  'codex',
  'monitor',
  'market',
  'logs',
  'settings',
] as const

export type InternalPageId = (typeof INTERNAL_PAGE_IDS)[number]

export interface InternalWorkspaceTab {
  id: `internal:${InternalPageId}`
  kind: 'internal'
  page: InternalPageId
}

export interface WebWorkspaceTab {
  id: string
  kind: 'web'
  title: string
  url: string
  reuseKey?: string
  source?: WebWorkspaceTabSource
}

export type WebWorkspaceTabSourceKind = 'market' | 'relay' | 'oauth' | 'manual'

export interface WebWorkspaceTabSource {
  kind: WebWorkspaceTabSourceKind
  id?: string
}

export interface OpenWebWorkspaceTabInput {
  url: string
  title: string
  reuseKey?: string
  source?: WebWorkspaceTabSource
}

export type WorkspaceTab = InternalWorkspaceTab | WebWorkspaceTab

export interface WorkspaceTabState {
  tabs: WorkspaceTab[]
  activeTabId: string
}

export type TabDropEdge = 'before' | 'after'

const STORAGE_KEY = 'aether:workspace-tabs:v1'

export function internalTabId(page: InternalPageId): InternalWorkspaceTab['id'] {
  return `internal:${page}`
}

export function createInternalTab(page: InternalPageId): InternalWorkspaceTab {
  return { id: internalTabId(page), kind: 'internal', page }
}

export function defaultWorkspaceTabState(): WorkspaceTabState {
  return {
    tabs: INTERNAL_PAGE_IDS.map(createInternalTab),
    activeTabId: internalTabId('upstreams'),
  }
}

export function loadWorkspaceTabState(): WorkspaceTabState {
  try {
    const parsed = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? 'null') as unknown
    return normalizeWorkspaceState(parsed) ?? defaultWorkspaceTabState()
  } catch {
    return defaultWorkspaceTabState()
  }
}

export function saveWorkspaceTabState(state: WorkspaceTabState) {
  try {
    const tabs = state.tabs.filter((tab): tab is InternalWorkspaceTab => tab.kind === 'internal')
    const activeTabId = tabs.some((tab) => tab.id === state.activeTabId)
      ? state.activeTabId
      : tabs[0]?.id
    if (!activeTabId) return
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify({ version: 1, tabs, activeTabId }))
  } catch {
    // The workspace remains usable when storage is unavailable.
  }
}

export function activeWorkspaceTab(state: WorkspaceTabState): WorkspaceTab {
  return state.tabs.find((tab) => tab.id === state.activeTabId) ?? state.tabs[0]
}

export function activateWorkspaceTab(
  state: WorkspaceTabState,
  tabId: string,
): WorkspaceTabState {
  if (tabId === state.activeTabId || !state.tabs.some((tab) => tab.id === tabId)) return state
  return { ...state, activeTabId: tabId }
}

export function openInternalWorkspaceTab(
  state: WorkspaceTabState,
  page: InternalPageId,
): WorkspaceTabState {
  const id = internalTabId(page)
  if (state.tabs.some((tab) => tab.id === id)) return activateWorkspaceTab(state, id)

  const activeIndex = state.tabs.findIndex((tab) => tab.id === state.activeTabId)
  const insertAt = activeIndex < 0 ? state.tabs.length : activeIndex + 1
  const tabs = [...state.tabs]
  tabs.splice(insertAt, 0, createInternalTab(page))
  return { tabs, activeTabId: id }
}

export function openWebWorkspaceTab(
  state: WorkspaceTabState,
  input: OpenWebWorkspaceTabInput,
): WorkspaceTabState {
  const reuseKey = input.reuseKey?.trim() || undefined
  if (reuseKey) {
    const existing = state.tabs.find(
      (tab): tab is WebWorkspaceTab => tab.kind === 'web' && tab.reuseKey === reuseKey,
    )
    if (existing) return activateWorkspaceTab(state, existing.id)
  }

  const tab: WebWorkspaceTab = {
    id: createWebTabId(),
    kind: 'web',
    title: input.title.trim() || webHost(input.url),
    url: input.url.trim(),
    reuseKey,
    source: input.source,
  }
  const activeIndex = state.tabs.findIndex((item) => item.id === state.activeTabId)
  const insertAt = activeIndex < 0 ? state.tabs.length : activeIndex + 1
  const tabs = [...state.tabs]
  tabs.splice(insertAt, 0, tab)
  return { tabs, activeTabId: tab.id }
}

export function closeWorkspaceTab(
  state: WorkspaceTabState,
  tabId: string,
): WorkspaceTabState {
  const tab = state.tabs.find((item) => item.id === tabId)
  if (!tab || tab.kind === 'internal') return state
  return removeWorkspaceTab(state, tabId)
}

export function hideInternalWorkspaceTab(
  state: WorkspaceTabState,
  tabId: string,
): WorkspaceTabState {
  const tab = state.tabs.find((item) => item.id === tabId)
  const visibleInternalCount = state.tabs.filter((item) => item.kind === 'internal').length
  if (!tab || tab.kind !== 'internal' || visibleInternalCount <= 1) return state
  return removeWorkspaceTab(state, tabId)
}

function removeWorkspaceTab(
  state: WorkspaceTabState,
  tabId: string,
): WorkspaceTabState {
  if (state.tabs.length <= 1) return state
  const closeIndex = state.tabs.findIndex((tab) => tab.id === tabId)
  if (closeIndex < 0) return state

  const tabs = state.tabs.filter((tab) => tab.id !== tabId)
  if (state.activeTabId !== tabId) return { ...state, tabs }

  const nextActive = tabs[Math.min(closeIndex, tabs.length - 1)]
  return { tabs, activeTabId: nextActive.id }
}

export function moveWorkspaceTab(
  state: WorkspaceTabState,
  sourceId: string,
  targetId: string,
  edge: TabDropEdge,
): WorkspaceTabState {
  if (sourceId === targetId) return state
  const source = state.tabs.find((tab) => tab.id === sourceId)
  if (!source || !state.tabs.some((tab) => tab.id === targetId)) return state

  const tabs = state.tabs.filter((tab) => tab.id !== sourceId)
  const targetIndex = tabs.findIndex((tab) => tab.id === targetId)
  tabs.splice(targetIndex + (edge === 'after' ? 1 : 0), 0, source)
  return { ...state, tabs }
}

export function cycleWorkspaceTab(
  state: WorkspaceTabState,
  direction: 1 | -1,
): WorkspaceTabState {
  if (state.tabs.length < 2) return state
  const activeIndex = Math.max(0, state.tabs.findIndex((tab) => tab.id === state.activeTabId))
  const nextIndex = (activeIndex + direction + state.tabs.length) % state.tabs.length
  return { ...state, activeTabId: state.tabs[nextIndex].id }
}

export function activateWorkspaceTabAt(
  state: WorkspaceTabState,
  index: number,
): WorkspaceTabState {
  const tab = state.tabs[index]
  return tab ? activateWorkspaceTab(state, tab.id) : state
}

function normalizeWorkspaceState(value: unknown): WorkspaceTabState | null {
  if (!isRecord(value) || value.version !== 1 || !Array.isArray(value.tabs)) return null

  const seen = new Set<string>()
  const tabs = value.tabs
    .map(normalizeWorkspaceTab)
    .filter((tab): tab is WorkspaceTab => {
      if (!tab || seen.has(tab.id)) return false
      seen.add(tab.id)
      return true
    })
    .slice(0, 24)

  if (!tabs.length) return null
  const activeTabId = typeof value.activeTabId === 'string'
    && tabs.some((tab) => tab.id === value.activeTabId)
    ? value.activeTabId
    : tabs[0].id
  return { tabs, activeTabId }
}

function normalizeWorkspaceTab(value: unknown): WorkspaceTab | null {
  if (!isRecord(value)) return null
  if (value.kind === 'internal' && isInternalPageId(value.page)) {
    return createInternalTab(value.page)
  }
  return null
}

function isInternalPageId(value: unknown): value is InternalPageId {
  return typeof value === 'string' && INTERNAL_PAGE_IDS.includes(value as InternalPageId)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function createWebTabId() {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return `web:${crypto.randomUUID()}`
  }
  return `web:${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`
}

function webHost(value: string) {
  try {
    return new URL(value).hostname
  } catch {
    return value
  }
}
