import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Activity, ScrollText, Server, Terminal } from 'lucide-react'
import { AccountTable } from './components/AccountTable'
import { AccountToolbar } from './components/AccountToolbar'
import { ApiKeyDialog } from './components/ApiKeyDialog'
import { AppHeader } from './components/AppHeader'
import { ClipboardImportDialog } from './components/ClipboardImportDialog'
import { CodexSettingsPanel } from './components/CodexSettingsPanel'
import { ChannelMonitorPanel } from './components/ChannelMonitorPanel'
import { DeleteAccountDialog } from './components/DeleteAccountDialog'
import { ImportDialog } from './components/ImportDialog'
import { LoggerPage } from './components/LoggerPage'
import { PageImportDropZone } from './components/PageImportDropZone'
import { ProxyPanel } from './components/ProxyPanel'
import { ResetAccessKeyDialog } from './components/ResetAccessKeyDialog'
import { ToastStack } from './components/ToastStack'
import { TooltipProvider } from './components/TooltipProvider'
import { TrashDialog } from './components/TrashDialog'
import {
  deleteAccount as deleteAccountCommand,
  discardClipboardImport,
  exportAccounts,
  confirmClipboardImport,
  getProxyInfo,
  getCodexSessionHistoryStatus,
  getCodexTakeoverStatus,
  inspectClipboardImport,
  listAccounts,
  migrateCodexSessionHistory,
  openRelaySite,
  queryAccountQuota,
  queryAllQuotas,
  queryRelayUsage as queryRelayUsageCommand,
  refreshAccount,
  refreshAllAccounts,
  resetRequestCounts,
  resetAccessToken,
  restoreCodexSessionHistory,
  setCodexTakeover,
  setAccountStatus,
  setAccountPriority,
  setAccountConcurrency,
  testAccount,
} from './lib/commands'
import { getChannelMonitorSnapshot, probeChannel } from './lib/channelMonitor'
import { errorText } from './lib/format'
import {
  loadQuotaCache,
  removeQuotaFromCache,
  saveQuotaBatchToCache,
  saveQuotaToCache,
} from './lib/quotaCache'
import {
  loadRelayUsageCache,
  removeRelayUsageFromCache,
  saveRelayUsageFailureToCache,
  saveRelayUsageToCache,
} from './lib/relayUsageCache'
import {
  DEFAULT_QUOTA_REFRESH_SETTINGS,
  loadQuotaRefreshSettings,
  QUOTA_REFRESH_INTERVALS,
  saveQuotaRefreshSettings,
  type QuotaRefreshInterval,
} from './lib/quotaRefreshSettings'
import type {
  Account,
  AccountQuotaResult,
  AccountStatus,
  AccountTypeFilter,
  CodexSessionHistoryStatus,
  CodexTakeoverStatus,
  ClipboardImportCandidate,
  ProxyInfo,
  QuotaQueryState,
  RelayUsageQueryState,
  ToastItem,
} from './types'
import type { ChannelMonitorSnapshot } from './monitorTypes'

const USAGE_QUERY_CONCURRENCY = 3

function advanceRequestEpoch(epochs: Map<string, number>, accountId: string) {
  const next = (epochs.get(accountId) ?? 0) + 1
  epochs.set(accountId, next)
  return next
}

export default function App() {
  const [accounts, setAccounts] = useState<Account[]>([])
  const [proxy, setProxy] = useState<ProxyInfo | null>(null)
  const [codexTakeover, setCodexTakeoverState] = useState<CodexTakeoverStatus | null>(null)
  const [codexSessionHistory, setCodexSessionHistory] = useState<CodexSessionHistoryStatus | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState('')
  const [query, setQuery] = useState('')
  const [statusFilter, setStatusFilter] = useState<'all' | AccountStatus>('all')
  const [typeFilter, setTypeFilter] = useState<AccountTypeFilter>('all')
  const [importOpen, setImportOpen] = useState(false)
  const [importFiles, setImportFiles] = useState<File[]>([])
  const [clipboardCandidate, setClipboardCandidate] = useState<ClipboardImportCandidate | null>(null)
  const [relayOpen, setRelayOpen] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<Account | null>(null)
  const [resetKeyOpen, setResetKeyOpen] = useState(false)
  const [trashOpen, setTrashOpen] = useState(false)
  const [activeTab, setActiveTab] = useState<'upstreams' | 'codex' | 'monitor' | 'logs'>('upstreams')
  const [busyActions, setBusyActions] = useState<Set<string>>(() => new Set())
  const [quotaStates, setQuotaStates] = useState<Record<string, QuotaQueryState>>({})
  const [relayUsageStates, setRelayUsageStates] = useState<Record<string, RelayUsageQueryState>>({})
  const [relayCacheHydrated, setRelayCacheHydrated] = useState(false)
  const [quotaAutoRefreshEnabled, setQuotaAutoRefreshEnabled] = useState(
    DEFAULT_QUOTA_REFRESH_SETTINGS.enabled,
  )
  const [quotaAutoRefreshInterval, setQuotaAutoRefreshInterval] = useState<QuotaRefreshInterval>(
    DEFAULT_QUOTA_REFRESH_SETTINGS.intervalMinutes,
  )
  const [quotaRefreshSettingsHydrated, setQuotaRefreshSettingsHydrated] = useState(false)
  const [monitorSnapshot, setMonitorSnapshot] = useState<ChannelMonitorSnapshot | null>(null)
  const [monitorLoading, setMonitorLoading] = useState(true)
  const [monitorRefreshing, setMonitorRefreshing] = useState(false)
  const [monitorError, setMonitorError] = useState('')
  const [probeBusy, setProbeBusy] = useState<Set<string>>(() => new Set())
  const [toasts, setToasts] = useState<ToastItem[]>([])
  const toastId = useRef(0)
  const relayAutoVersions = useRef(new Map<string, string>())
  const relayFreshCacheIds = useRef(new Set<string>())
  const quotaRequestEpochs = useRef(new Map<string, number>())
  const relayRequestEpochs = useRef(new Map<string, number>())
  const quotaQueryInFlight = useRef(false)
  const clipboardScanBusy = useRef(false)
  const dialogOpenRef = useRef(false)
  const monitorLoadInFlight = useRef(false)

  const notify = useCallback((message: string, error = false) => {
    const id = ++toastId.current
    setToasts((current) => [...current, { id, message, error }])
    window.setTimeout(
      () => setToasts((current) => current.filter((item) => item.id !== id)),
      3500,
    )
  }, [])

  const loadProxy = useCallback(async () => {
    const nextProxy = await getProxyInfo()
    setProxy((current) => sameProxyInfo(current, nextProxy) ? current : nextProxy)
  }, [])

  const loadCodexTakeover = useCallback(async () => {
    const status = await getCodexTakeoverStatus()
    setCodexTakeoverState(status)
  }, [])

  const loadCodexSessionHistory = useCallback(async () => {
    const status = await getCodexSessionHistoryStatus()
    setCodexSessionHistory(status)
  }, [])

  const loadChannelMonitor = useCallback(async (initial = false) => {
    if (monitorLoadInFlight.current) return
    monitorLoadInFlight.current = true
    if (initial) setMonitorLoading(true)
    else setMonitorRefreshing(true)
    try {
      setMonitorSnapshot(await getChannelMonitorSnapshot())
      setMonitorError('')
    } catch (error) {
      setMonitorError(errorText(error))
    } finally {
      monitorLoadInFlight.current = false
      setMonitorLoading(false)
      setMonitorRefreshing(false)
    }
  }, [])

  const loadAccounts = useCallback(async () => {
    setAccounts(await listAccounts())
    setLoadError('')
  }, [])

  const refreshData = useCallback(async () => {
    await loadAccounts()
    await loadProxy().catch(() => undefined)
    await loadCodexTakeover().catch(() => undefined)
    await loadCodexSessionHistory().catch(() => undefined)
  }, [loadAccounts, loadProxy, loadCodexTakeover, loadCodexSessionHistory])

  const setActionBusy = useCallback((key: string, busy: boolean) => {
    setBusyActions((current) => {
      const next = new Set(current)
      if (busy) next.add(key)
      else next.delete(key)
      return next
    })
  }, [])

  const retryLoad = useCallback(() => {
    setLoading(true)
    refreshData()
      .catch((error) => {
        const message = errorText(error)
        setLoadError(message)
        notify(message, true)
      })
      .finally(() => setLoading(false))
  }, [refreshData, notify])

  useEffect(() => {
    retryLoad()
  }, [retryLoad])

  // Restore last-known values before deciding which relay accounts need a refresh.
  useEffect(() => {
    let disposed = false

    void loadQuotaCache()
      .then(({ states }) => {
        if (disposed) return
        const untouchedStates = Object.fromEntries(
          Object.entries(states).filter(([id]) => !quotaRequestEpochs.current.has(id)),
        )
        if (Object.keys(untouchedStates).length) {
          setQuotaStates((current) => ({ ...untouchedStates, ...current }))
        }
      })
      .catch(() => undefined)

    void loadRelayUsageCache()
      .then(({ states, staleAccountIds }) => {
        if (disposed) return
        const staleIds = new Set(staleAccountIds)
        const untouchedStates = Object.fromEntries(
          Object.entries(states).filter(([id]) => !relayRequestEpochs.current.has(id)),
        )
        for (const id of Object.keys(untouchedStates)) {
          if (!staleIds.has(id)) relayFreshCacheIds.current.add(id)
        }
        if (Object.keys(untouchedStates).length) {
          setRelayUsageStates((current) => ({ ...untouchedStates, ...current }))
        }
      })
      .catch(() => undefined)
      .finally(() => {
        if (!disposed) setRelayCacheHydrated(true)
      })

    return () => { disposed = true }
  }, [])

  useEffect(() => {
    let disposed = false
    void loadQuotaRefreshSettings().then((settings) => {
      if (disposed) return
      setQuotaAutoRefreshEnabled(settings.enabled)
      setQuotaAutoRefreshInterval(settings.intervalMinutes)
      setQuotaRefreshSettingsHydrated(true)
    })
    return () => { disposed = true }
  }, [])

  useEffect(() => {
    if (!quotaRefreshSettingsHydrated) return
    void saveQuotaRefreshSettings({
      enabled: quotaAutoRefreshEnabled,
      intervalMinutes: quotaAutoRefreshInterval,
    })
  }, [quotaAutoRefreshEnabled, quotaAutoRefreshInterval, quotaRefreshSettingsHydrated])

  useEffect(() => {
    dialogOpenRef.current = Boolean(
      importOpen || relayOpen || deleteTarget || resetKeyOpen || clipboardCandidate,
    )
  }, [importOpen, relayOpen, deleteTarget, resetKeyOpen, clipboardCandidate])

  const scanClipboard = useCallback(async () => {
    if (clipboardScanBusy.current || dialogOpenRef.current) return
    clipboardScanBusy.current = true
    try {
      const candidate = await inspectClipboardImport()
      if (candidate) setClipboardCandidate(candidate)
    } catch {
      // Clipboard access and unrelated contents are intentionally silent.
    } finally {
      clipboardScanBusy.current = false
    }
  }, [])

  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const clipboardPreview = import.meta.env.DEV
      && params.has('preview')
      && params.has('clipboard')

    if (clipboardPreview) {
      void scanClipboard()
      return
    }
    if (!('__TAURI_INTERNALS__' in window)) return

    let disposed = false
    let unlisten: (() => void) | undefined
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused) void scanClipboard()
      })
      .then((stopListening) => {
        if (disposed) stopListening()
        else unlisten = stopListening
      })
      .catch(() => undefined)

    void scanClipboard()
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [scanClipboard])

  useEffect(() => {
    let inFlight = false
    const refreshProxyStats = () => {
      if (inFlight || document.visibilityState === 'hidden') return
      inFlight = true
      void loadProxy()
        .catch(() => undefined)
        .finally(() => { inFlight = false })
    }
    const timer = window.setInterval(refreshProxyStats, 1000)
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') refreshProxyStats()
    }
    document.addEventListener('visibilitychange', refreshWhenVisible)
    window.addEventListener('focus', refreshProxyStats)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refreshWhenVisible)
      window.removeEventListener('focus', refreshProxyStats)
    }
  }, [loadProxy])

  useEffect(() => {
    if (activeTab !== 'monitor') return
    const refreshMonitor = () => {
      if (document.visibilityState === 'visible') void loadChannelMonitor(false)
    }
    void loadChannelMonitor(true)
    const timer = window.setInterval(refreshMonitor, 10_000)
    document.addEventListener('visibilitychange', refreshMonitor)
    return () => {
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', refreshMonitor)
    }
  }, [activeTab, loadChannelMonitor])

  const visibleAccounts = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase()
    return accounts.filter((account) => {
      if (statusFilter !== 'all' && account.status !== statusFilter) return false
      if (typeFilter !== 'all' && account.account_type !== typeFilter) return false
      if (!normalizedQuery) return true
      return [
        account.name,
        account.email,
        account.plan_type,
        account.credential_masked,
        account.base_url,
        account.models?.join(' '),
      ].some((value) => value?.toLowerCase().includes(normalizedQuery))
    })
  }, [accounts, query, statusFilter, typeFilter])

  const activeAccountCount = useMemo(
    () => accounts.filter((account) => account.status === 'active').length,
    [accounts],
  )

  const errorAccounts = useMemo(
    () => accounts.filter((account) => account.last_error),
    [accounts],
  )

  const copyText = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value)
      notify('已复制')
    } catch {
      notify('复制失败', true)
    }
  }

  const runChannelProbe = useCallback(async (accountId: string) => {
    setProbeBusy((current) => new Set(current).add(accountId))
    try {
      notify(await probeChannel(accountId))
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      await loadChannelMonitor(false)
      setProbeBusy((current) => {
        const next = new Set(current)
        next.delete(accountId)
        return next
      })
    }
  }, [loadChannelMonitor, notify])

  const openImport = useCallback((files: File[] = []) => {
    setImportFiles(files)
    setImportOpen(true)
  }, [])

  const closeImport = useCallback(() => {
    setImportOpen(false)
    setImportFiles([])
  }, [])

  const closeClipboardImport = useCallback(() => {
    if (!clipboardCandidate) return
    const candidateId = clipboardCandidate.candidate_id
    setClipboardCandidate(null)
    void discardClipboardImport(candidateId).catch(() => undefined)
  }, [clipboardCandidate])

  const importClipboardCandidate = async () => {
    if (!clipboardCandidate) return
    setActionBusy('clipboard-import', true)
    try {
      const result = await confirmClipboardImport(clipboardCandidate.candidate_id)
      if (result.created || result.updated) await refreshData()
      setClipboardCandidate(null)
      const summary = `新增 ${result.created}，更新 ${result.updated}`
      if (result.failed) {
        const detail = result.errors[0]?.message
        notify(`${summary}，失败 ${result.failed}${detail ? `：${detail}` : ''}`, true)
      } else {
        notify(`剪贴板账号已导入：${summary}`)
      }
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy('clipboard-import', false)
    }
  }

  const runAccountAction = async (action: 'toggle' | 'test' | 'refresh', account: Account) => {
    const actionKey = `${action}:${account.id}`
    setActionBusy(actionKey, true)
    try {
      if (action === 'toggle') {
        const nextStatus = account.status === 'active' ? 'disabled' : 'active'
        const updated = await setAccountStatus(account.id, nextStatus)
        if (!updated) throw new Error('上游不存在')
        notify(nextStatus === 'active' ? '上游已启用' : '上游已停用')
      } else if (action === 'test') {
        notify(await testAccount(account.id))
      } else {
        await refreshAccount(account.id)
        notify('OAuth 凭据已刷新')
      }
      await refreshData()
    } catch (error) {
      notify(errorText(error), true)
      await refreshData().catch(() => undefined)
    } finally {
      setActionBusy(actionKey, false)
    }
  }

  const openRelayWebsite = async (account: Account) => {
    const actionKey = `open-relay:${account.id}`
    setActionBusy(actionKey, true)
    try {
      await openRelaySite(account.id)
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy(actionKey, false)
    }
  }

  const deleteSelectedAccount = async () => {
    if (!deleteTarget) return
    const actionKey = `delete:${deleteTarget.id}`
    setActionBusy(actionKey, true)
    try {
      const deleted = await deleteAccountCommand(deleteTarget.id)
      if (!deleted) throw new Error('上游不存在')
      setQuotaStates((current) => {
        const next = { ...current }
        delete next[deleteTarget.id]
        return next
      })
      setRelayUsageStates((current) => {
        const next = { ...current }
        delete next[deleteTarget.id]
        return next
      })
      advanceRequestEpoch(quotaRequestEpochs.current, deleteTarget.id)
      advanceRequestEpoch(relayRequestEpochs.current, deleteTarget.id)
      relayFreshCacheIds.current.delete(deleteTarget.id)
      await Promise.all([
        removeQuotaFromCache(deleteTarget.id),
        removeRelayUsageFromCache(deleteTarget.id),
      ])
      relayAutoVersions.current.delete(deleteTarget.id)
      setDeleteTarget(null)
      notify('上游已删除')
      await refreshData()
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy(actionKey, false)
    }
  }

  const updatePriority = async (account: Account, priority: number) => {
    const actionKey = `priority:${account.id}`
    setActionBusy(actionKey, true)
    try {
      const updated = await setAccountPriority(account.id, priority)
      if (!updated) throw new Error('上游不存在')
      await refreshData()
      notify('优先级已更新')
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy(actionKey, false)
    }
  }

  const updateConcurrency = async (account: Account, concurrency: number) => {
    const actionKey = `concurrency:${account.id}`
    setActionBusy(actionKey, true)
    try {
      const updated = await setAccountConcurrency(account.id, concurrency)
      if (!updated) throw new Error('上游不存在')
      await refreshData()
      notify('并发容量已更新')
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy(actionKey, false)
    }
  }

  const refreshAll = async () => {
    setActionBusy('refresh-all', true)
    try {
      const result = await refreshAllAccounts()
      notify(
        result.failed
          ? `刷新完成，${result.failed} 个失败`
          : `已刷新 ${result.updated} 个 OAuth 上游`,
        result.failed > 0,
      )
      await refreshData()
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy('refresh-all', false)
    }
  }

  const queryQuota = async (account: Account) => {
    const requestEpoch = advanceRequestEpoch(quotaRequestEpochs.current, account.id)
    setQuotaStates((current) => ({
      ...current,
      [account.id]: { status: 'loading' },
    }))
    try {
      const quota = await queryAccountQuota(account.id)
      if (quotaRequestEpochs.current.get(account.id) !== requestEpoch) return
      setQuotaStates((current) => ({
        ...current,
        [account.id]: { status: 'success', quota },
      }))
      await saveQuotaToCache(account.id, quota)
    } catch (error) {
      if (quotaRequestEpochs.current.get(account.id) !== requestEpoch) return
      setQuotaStates((current) => ({
        ...current,
        [account.id]: { status: 'error', error: errorText(error) },
      }))
    }
  }

  const queryRelayUsage = useCallback(async (account: Account, background = false) => {
    const requestEpoch = advanceRequestEpoch(relayRequestEpochs.current, account.id)
    setRelayUsageStates((current) => {
      if (background && current[account.id]?.status === 'success') return current
      return {
        ...current,
        [account.id]: { status: 'loading' },
      }
    })
    try {
      const usage = await queryRelayUsageCommand(account.id)
      if (relayRequestEpochs.current.get(account.id) !== requestEpoch) return false
      setRelayUsageStates((current) => ({
        ...current,
        [account.id]: { status: 'success', usage },
      }))
      await saveRelayUsageToCache(account.id, usage)
      if (relayRequestEpochs.current.get(account.id) !== requestEpoch) return false
      relayFreshCacheIds.current.add(account.id)
      return true
    } catch (error) {
      if (relayRequestEpochs.current.get(account.id) !== requestEpoch) return false
      const message = errorText(error)
      setRelayUsageStates((current) => {
        if (background && current[account.id]?.status === 'success') return current
        return {
          ...current,
          [account.id]: { status: 'error', error: message },
        }
      })
      await saveRelayUsageFailureToCache(account.id, message)
      if (relayRequestEpochs.current.get(account.id) === requestEpoch) {
        relayFreshCacheIds.current.add(account.id)
      }
      return false
    }
  }, [])

  useEffect(() => {
    if (!relayCacheHydrated) return
    const pendingAccounts = accounts
      .filter((account) => account.account_type === 'api_key' && account.status === 'active')
      .filter((account) => {
        const version = account.updated_at || account.created_at
        const previousVersion = relayAutoVersions.current.get(account.id)
        if (previousVersion === version) return false
        relayAutoVersions.current.set(account.id, version)
        if (previousVersion !== undefined) return true
        return !relayRequestEpochs.current.has(account.id)
          && !relayFreshCacheIds.current.has(account.id)
      })
    void runAsyncPool(
      pendingAccounts,
      USAGE_QUERY_CONCURRENCY,
      (account) => queryRelayUsage(account, true),
    )
  }, [accounts, queryRelayUsage, relayCacheHydrated])

  const queryEveryQuota = useCallback(async (automatic = false) => {
    const oauthAccounts = accounts.filter(
      (account) => account.account_type === 'oauth' && account.status === 'active',
    )
    const relayAccounts = accounts.filter(
      (account) => account.account_type === 'api_key' && account.status === 'active',
    )
    if (!oauthAccounts.length && !relayAccounts.length) {
      if (!automatic) notify('暂无启用的上游')
      return
    }
    if (quotaQueryInFlight.current) return

    quotaQueryInFlight.current = true
    setActionBusy('quota-all', true)
    const quotaEpochs = new Map(
      oauthAccounts.map((account) => [
        account.id,
        advanceRequestEpoch(quotaRequestEpochs.current, account.id),
      ]),
    )
    setQuotaStates((current) => {
      const next = { ...current }
      oauthAccounts.forEach((account) => { next[account.id] = { status: 'loading' } })
      return next
    })

    try {
      let failed = 0
      if (oauthAccounts.length) {
        try {
          const results = await queryAllQuotas()
          const nextStates: Record<string, QuotaQueryState> = {}
          const cacheUpdates: {
            accountId: string
            quota: NonNullable<AccountQuotaResult['quota']>
          }[] = []

          oauthAccounts.forEach((account) => {
            if (quotaRequestEpochs.current.get(account.id) !== quotaEpochs.get(account.id)) return
            const result = findQuotaResult(results, account)
            if (result?.quota) {
              nextStates[account.id] = { status: 'success', quota: result.quota }
              cacheUpdates.push({ accountId: account.id, quota: result.quota })
            } else {
              nextStates[account.id] = {
                status: 'error',
                error: result?.error || '未返回额度结果',
              }
              failed++
            }
          })

          if (Object.keys(nextStates).length) {
            setQuotaStates((current) => ({ ...current, ...nextStates }))
          }
          await saveQuotaBatchToCache(cacheUpdates)
        } catch (error) {
          const message = errorText(error)
          const currentAccounts = oauthAccounts.filter(
            (account) => quotaRequestEpochs.current.get(account.id) === quotaEpochs.get(account.id),
          )
          failed += currentAccounts.length
          setQuotaStates((current) => {
            const next = { ...current }
            currentAccounts.forEach((account) => {
              next[account.id] = { status: 'error', error: message }
            })
            return next
          })
        }
      }

      const relayResults = await runAsyncPool(
        relayAccounts,
        USAGE_QUERY_CONCURRENCY,
        (account) => queryRelayUsage(account),
      )
      failed += relayResults.filter((success) => !success).length
      if (!automatic || failed) {
        notify(
          automatic
            ? `自动刷新完成，${failed} 个失败`
            : failed ? `用量查询完成，${failed} 个失败` : '全部用量已更新',
          failed > 0,
        )
      }
    } catch (error) {
      notify(
        automatic ? `自动刷新失败：${errorText(error)}` : errorText(error),
        true,
      )
    } finally {
      quotaQueryInFlight.current = false
      setActionBusy('quota-all', false)
    }
  }, [accounts, notify, queryRelayUsage, setActionBusy])

  useEffect(() => {
    if (!quotaRefreshSettingsHydrated || !quotaAutoRefreshEnabled) return
    const timer = window.setInterval(
      () => { void queryEveryQuota(true) },
      quotaAutoRefreshInterval * 60_000,
    )
    return () => window.clearInterval(timer)
  }, [
    queryEveryQuota,
    quotaAutoRefreshEnabled,
    quotaAutoRefreshInterval,
    quotaRefreshSettingsHydrated,
  ])

  const exportBackup = async () => {
    try {
      const content = await exportAccounts()
      const url = URL.createObjectURL(new Blob([content], { type: 'application/json' }))
      const link = document.createElement('a')
      link.href = url
      link.download = `aether-backup-${new Date().toISOString().slice(0, 10)}.json`
      link.click()
      window.setTimeout(() => URL.revokeObjectURL(url), 1000)
      notify('备份已导出')
    } catch (error) {
      notify(errorText(error), true)
    }
  }

  const removeErrorAccounts = async () => {
    const targets = accounts.filter((account) => account.last_error)
    if (!targets.length) {
      notify('没有报错上游')
      return
    }
    setActionBusy('remove-errors', true)
    let removed = 0
    try {
      for (const account of targets) {
        const deleted = await deleteAccountCommand(account.id)
        if (deleted) {
          removed++
          setQuotaStates((current) => {
            const next = { ...current }
            delete next[account.id]
            return next
          })
          setRelayUsageStates((current) => {
            const next = { ...current }
            delete next[account.id]
            return next
          })
          advanceRequestEpoch(quotaRequestEpochs.current, account.id)
          advanceRequestEpoch(relayRequestEpochs.current, account.id)
          relayFreshCacheIds.current.delete(account.id)
          await Promise.all([
            removeQuotaFromCache(account.id),
            removeRelayUsageFromCache(account.id),
          ])
          relayAutoVersions.current.delete(account.id)
        }
      }
      notify(`已移除 ${removed} 个报错上游`)
      await refreshData()
    } catch (error) {
      notify(errorText(error), true)
      await refreshData().catch(() => undefined)
    } finally {
      setActionBusy('remove-errors', false)
    }
  }

  const resetCounts = async () => {
    setActionBusy('reset-counts', true)
    try {
      await resetRequestCounts()
      notify('请求计数已清空')
      await refreshData()
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy('reset-counts', false)
    }
  }

  const resetProxyAccessToken = async () => {
    setActionBusy('reset-access-token', true)
    try {
      const accessToken = await resetAccessToken()
      setProxy((current) => current ? { ...current, access_token: accessToken } : current)
      await loadCodexTakeover().catch(() => undefined)
      await loadCodexSessionHistory().catch(() => undefined)
      setResetKeyOpen(false)
      notify('API Key 已重置')
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy('reset-access-token', false)
    }
  }

  const toggleCodexTakeover = async () => {
    const shouldEnable = !(codexTakeover?.active || codexTakeover?.backup_available)
    setActionBusy('codex-takeover', true)
    try {
      const status = await setCodexTakeover(shouldEnable)
      setCodexTakeoverState(status)
      await loadCodexSessionHistory().catch(() => undefined)
      notify(shouldEnable ? 'Codex 已接管到 Aether' : 'Codex 配置已恢复')
    } catch (error) {
      notify(errorText(error), true)
    } finally {
      setActionBusy('codex-takeover', false)
    }
  }

  const migrateCodexSessionHistoryAction = async () => {
    setActionBusy('codex-history-migrate', true)
    try {
      const result = await migrateCodexSessionHistory()
      await loadCodexSessionHistory().catch(() => undefined)
      notify(result.skipped_reason
        ? codexHistorySkipMessage(result.skipped_reason)
        : `已迁移 ${result.migrated_jsonl_files} 个会话文件、${result.migrated_state_rows} 条会话索引`)
    } catch (error) {
      notify(errorText(error), true)
      await loadCodexSessionHistory().catch(() => undefined)
    } finally {
      setActionBusy('codex-history-migrate', false)
    }
  }

  const restoreCodexSessionHistoryAction = async () => {
    setActionBusy('codex-history-restore', true)
    try {
      const result = await restoreCodexSessionHistory()
      await loadCodexSessionHistory().catch(() => undefined)
      notify(result.skipped_reason
        ? codexHistorySkipMessage(result.skipped_reason)
        : `已恢复 ${result.restored_jsonl_files} 个会话文件、${result.restored_state_rows} 条会话索引`)
    } catch (error) {
      notify(errorText(error), true)
      await loadCodexSessionHistory().catch(() => undefined)
    } finally {
      setActionBusy('codex-history-restore', false)
    }
  }

  const deleteBusy = Boolean(
    deleteTarget && busyActions.has(`delete:${deleteTarget.id}`),
  )

  return (
    <>
      <TooltipProvider />
      <AppHeader proxy={proxy} onSecretAction={() => setTrashOpen(true)} />
      <nav className="tab-bar">
        <button
          className={`tab-item${activeTab === 'upstreams' ? ' active' : ''}`}
          onClick={() => setActiveTab('upstreams')}
        >
          <Server size={14} />
          上游管理
        </button>
        <button
          className={`tab-item${activeTab === 'codex' ? ' active' : ''}`}
          onClick={() => setActiveTab('codex')}
        >
          <Terminal size={14} />
          Codex 配置
        </button>
        <button
          className={`tab-item${activeTab === 'monitor' ? ' active' : ''}`}
          onClick={() => setActiveTab('monitor')}
        >
          <Activity size={14} />
          渠道监控
        </button>
        <button
          className={`tab-item${activeTab === 'logs' ? ' active' : ''}`}
          onClick={() => setActiveTab('logs')}
        >
          <ScrollText size={14} />
          请求日志
        </button>
      </nav>
      {activeTab === 'upstreams' ? (
      <main>
        <ProxyPanel
          proxy={proxy}
          activeAccountCount={activeAccountCount}
          accountCount={accounts.length}
          accounts={accounts}
          quotaStates={quotaStates}
          relayUsageStates={relayUsageStates}
          resetBusy={busyActions.has('reset-counts')}
          onResetCounts={resetCounts}
        />
        <PageImportDropZone onFiles={openImport} />

        <AccountToolbar
          query={query}
          statusFilter={statusFilter}
          typeFilter={typeFilter}
          refreshAllBusy={busyActions.has('refresh-all')}
          queryAllBusy={busyActions.has('quota-all')}
          autoRefreshEnabled={quotaAutoRefreshEnabled}
          autoRefreshIntervalMinutes={quotaAutoRefreshInterval}
          removeErrorsBusy={busyActions.has('remove-errors')}
          errorCount={errorAccounts.length}
          onQueryChange={setQuery}
          onStatusFilterChange={setStatusFilter}
          onTypeFilterChange={setTypeFilter}
          onImport={openImport}
          onRefreshAll={refreshAll}
          onQueryAll={() => { void queryEveryQuota(false) }}
          onAutoRefreshEnabledChange={setQuotaAutoRefreshEnabled}
          onAutoRefreshIntervalChange={(minutes) => {
            const interval = QUOTA_REFRESH_INTERVALS.find((value) => value === minutes)
            if (interval !== undefined) setQuotaAutoRefreshInterval(interval)
          }}
          onExport={exportBackup}
          onAddRelay={() => setRelayOpen(true)}
          onRemoveErrors={removeErrorAccounts}
        />

        <AccountTable
          accounts={visibleAccounts}
          hasAccounts={accounts.length > 0}
          loading={loading}
          loadError={loadError}
          busyActions={busyActions}
          quotaStates={quotaStates}
          relayUsageStates={relayUsageStates}
          accountCapacities={proxy?.account_capacities ?? {}}
          onRetry={retryLoad}
          onToggle={(account) => runAccountAction('toggle', account)}
          onTest={(account) => runAccountAction('test', account)}
          onOpenRelay={openRelayWebsite}
          onRefresh={(account) => runAccountAction('refresh', account)}
          onQuota={queryQuota}
          onRelayUsage={queryRelayUsage}
          onPriority={updatePriority}
          onConcurrency={updateConcurrency}
          onDelete={setDeleteTarget}
        />
      </main>
      ) : activeTab === 'codex' ? (
      <CodexSettingsPanel
        proxy={proxy}
        status={codexTakeover}
        sessionHistory={codexSessionHistory}
        busy={busyActions.has('codex-takeover')}
        migrateHistoryBusy={busyActions.has('codex-history-migrate')}
        restoreHistoryBusy={busyActions.has('codex-history-restore')}
        resetTokenBusy={busyActions.has('reset-access-token')}
        onCopy={copyText}
        onToggleTakeover={toggleCodexTakeover}
        onMigrateHistory={migrateCodexSessionHistoryAction}
        onRestoreHistory={restoreCodexSessionHistoryAction}
        onResetAccessToken={() => setResetKeyOpen(true)}
      />
      ) : activeTab === 'logs' ? (
      <LoggerPage />
      ) : (
      <main className="monitor-page">
        <ChannelMonitorPanel
          snapshot={monitorSnapshot}
          loading={monitorLoading}
          refreshing={monitorRefreshing}
          error={monitorError}
          probeBusy={probeBusy}
          onRefresh={() => { void loadChannelMonitor(false) }}
          onProbe={(accountId) => { void runChannelProbe(accountId) }}
        />
      </main>
      )}

      <ImportDialog
        open={importOpen}
        initialFiles={importFiles}
        onClose={closeImport}
        onImported={refreshData}
        notify={notify}
      />
      <ClipboardImportDialog
        candidate={clipboardCandidate}
        busy={busyActions.has('clipboard-import')}
        onClose={closeClipboardImport}
        onConfirm={importClipboardCandidate}
      />
      <ApiKeyDialog
        open={relayOpen}
        onClose={() => setRelayOpen(false)}
        onSaved={refreshData}
        notify={notify}
      />
      <DeleteAccountDialog
        account={deleteTarget}
        busy={deleteBusy}
        onClose={() => setDeleteTarget(null)}
        onConfirm={deleteSelectedAccount}
      />
      <ResetAccessKeyDialog
        open={resetKeyOpen}
        busy={busyActions.has('reset-access-token')}
        onClose={() => setResetKeyOpen(false)}
        onConfirm={resetProxyAccessToken}
      />
      <TrashDialog
        open={trashOpen}
        onClose={() => setTrashOpen(false)}
        onRestored={refreshData}
        notify={notify}
      />
      <ToastStack items={toasts} />
    </>
  )
}

async function runAsyncPool<T, R>(
  items: T[],
  concurrency: number,
  task: (item: T, index: number) => Promise<R>,
) {
  const results = new Array<R>(items.length)
  const workerCount = Math.min(items.length, Math.max(1, Math.trunc(concurrency)))
  let nextIndex = 0

  const worker = async () => {
    while (nextIndex < items.length) {
      const index = nextIndex
      nextIndex += 1
      results[index] = await task(items[index], index)
    }
  }

  await Promise.all(Array.from({ length: workerCount }, worker))
  return results
}

function findQuotaResult(results: AccountQuotaResult[], account: Account) {
  return results.find((result) => result.account_id === account.id)
}

function codexHistorySkipMessage(reason: string) {
  switch (reason) {
    case 'not_unified':
      return '请先接管 Codex，再迁移既有会话'
    case 'nothing_to_migrate':
      return '没有需要迁移的 Codex 会话'
    case 'no_backup_ledger':
      return '没有可恢复的官方会话备份'
    case 'nothing_to_restore':
      return '没有需要恢复的官方会话'
    default:
      return `Codex 会话操作已跳过：${reason}`
  }
}

function sameProxyInfo(current: ProxyInfo | null, next: ProxyInfo) {
  return current !== null
    && current.port === next.port
    && current.proxy_profile === next.proxy_profile
    && current.base_url === next.base_url
    && current.access_token === next.access_token
    && current.running === next.running
    && current.account_count === next.account_count
    && current.active_account_count === next.active_account_count
    && current.total_requests === next.total_requests
    && current.input_tokens === next.input_tokens
    && current.output_tokens === next.output_tokens
    && current.cached_tokens === next.cached_tokens
    && current.cache_write_tokens === next.cache_write_tokens
    && current.reasoning_tokens === next.reasoning_tokens
    && current.unpriced_tokens === next.unpriced_tokens
    && current.total_tokens === next.total_tokens
    && current.total_cost === next.total_cost
    && current.pricing_updated_at === next.pricing_updated_at
    && current.pricing_source === next.pricing_source
    && sameNumberMap(current.account_capacities, next.account_capacities)
}

function sameNumberMap(
  current: Record<string, number> | null | undefined,
  next: Record<string, number> | null | undefined,
) {
  const currentMap = current ?? {}
  const nextMap = next ?? {}
  const currentKeys = Object.keys(currentMap)
  const nextKeys = Object.keys(nextMap)
  return currentKeys.length === nextKeys.length
    && currentKeys.every((key) => currentMap[key] === nextMap[key])
}
