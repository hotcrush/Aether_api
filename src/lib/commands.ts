import { invoke } from '@tauri-apps/api/core'
import type {
  Account,
  AccountQuota,
  AccountQuotaResult,
  AccountStatus,
  AppVersion,
  CodexSessionHistoryMigrationResult,
  CodexSessionHistoryRestoreResult,
  CodexSessionHistoryStatus,
  CodexTakeoverStatus,
  ClipboardImportCandidate,
  ImportResult,
  ProxyInfo,
  RelayUsageSummary,
  RequestLog,
  RequestLogPage,
  RequestLogQuery,
} from '../types'

const previewMode = import.meta.env.DEV
  && typeof window !== 'undefined'
  && new URLSearchParams(window.location.search).has('preview')

const PREVIEW_CACHE_PREFIX = 'aether:preview-cache:'

let previewAccounts: Account[] = [
  {
    id: 'oauth-pro',
    name: '日常开发',
    account_type: 'oauth',
    credential_masked: 'eyJhbGci...8nQ',
    refreshable: true,
    base_url: '',
    chatgpt_account_id: 'account-pro-upstream',
    chatgpt_user_id: 'user-pro',
    email: 'dev@example.com',
    plan_type: 'plus',
    expires_at: Math.floor(Date.now() / 1000) + 86_400 * 6,
    priority: 0,
    models: [],
    weight: 1,
    concurrency: 10,
    status: 'active',
    last_error: '',
    last_used_at: '今天 20:48',
    request_count: 12842,
    created_at: '2026-07-02T10:00:00Z',
    updated_at: '2026-07-30T12:48:00Z',
  },
  {
    id: 'oauth-team',
    name: '团队备用',
    account_type: 'oauth',
    credential_masked: 'eyJhbGci...3kP',
    refreshable: true,
    base_url: '',
    chatgpt_account_id: 'account-team-upstream',
    chatgpt_user_id: 'user-team',
    email: 'ops@example.com',
    plan_type: 'team',
    expires_at: Math.floor(Date.now() / 1000) + 86_400 * 2,
    priority: 2,
    models: [],
    weight: 1,
    concurrency: 6,
    status: 'active',
    last_error: '',
    last_used_at: '昨天 18:20',
    request_count: 3761,
    created_at: '2026-07-12T10:00:00Z',
    updated_at: '2026-07-29T10:20:00Z',
  },
  {
    id: 'api-key',
    name: '团队中转站',
    account_type: 'api_key',
    credential_masked: 'sk-proj-****Q7K2',
    refreshable: false,
    base_url: 'https://gateway.example.com/v1',
    chatgpt_account_id: '',
    chatgpt_user_id: '',
    email: '',
    plan_type: '',
    expires_at: null,
    priority: 5,
    models: ['gpt-5', 'gpt-5-mini', 'o3'],
    weight: 3,
    concurrency: 20,
    status: 'active',
    last_error: '',
    last_used_at: '07-28 09:16',
    request_count: 642,
    created_at: '2026-07-18T10:00:00Z',
    updated_at: '2026-07-28T01:16:00Z',
  },
  {
    id: 'oauth-disabled',
    name: '归档账号池',
    account_type: 'oauth',
    credential_masked: 'eyJhbGci...9tD',
    refreshable: true,
    base_url: '',
    chatgpt_account_id: 'account-disabled-upstream',
    chatgpt_user_id: 'user-disabled',
    email: 'archive@example.com',
    plan_type: 'free',
    expires_at: null,
    priority: 9,
    models: [],
    weight: 1,
    concurrency: 4,
    status: 'disabled',
    last_error: '上次测试：OAuth token 已失效',
    last_used_at: null,
    request_count: 18,
    created_at: '2026-06-01T10:00:00Z',
    updated_at: '2026-07-20T10:00:00Z',
  },
]

let previewAccessToken = 'sk-local-cf4456e6195e4461957af12029f7cdfb'
let previewCodexTakeoverActive = false
let previewCodexHistoryBackupAvailable = false

const previewLogTimestamp = (offsetMs: number) => new Date(Date.now() - offsetMs).toISOString()

let previewRequestLogs: RequestLog[] = [
  {
    id: 8,
    request_id: 'req-preview-stream',
    attempt_index: 1,
    account_id: 'oauth-pro',
    account_name: '日常开发',
    account_type: 'oauth',
    source: 'proxy',
    method: 'POST',
    path: '/v1/responses',
    endpoint_family: 'responses',
    model: 'gpt-5.6-sol',
    status: 'pending',
    http_status: 200,
    streaming: true,
    ttfb_ms: 426,
    duration_ms: null,
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: 0,
    unpriced_tokens: 0,
    estimated_cost: 0,
    message: '',
    created_at: previewLogTimestamp(8_000),
    completed_at: null,
  },
  {
    id: 7,
    request_id: 'req-preview-retry',
    attempt_index: 2,
    account_id: 'api-key',
    account_name: '团队中转站',
    account_type: 'api_key',
    source: 'proxy',
    method: 'POST',
    path: '/v1/responses',
    endpoint_family: 'responses',
    model: 'gpt-5.5',
    status: 'success',
    http_status: 200,
    streaming: true,
    ttfb_ms: 612,
    duration_ms: 4_286,
    input_tokens: 18_420,
    output_tokens: 2_816,
    cached_tokens: 12_160,
    cache_write_tokens: 0,
    reasoning_tokens: 1_304,
    total_tokens: 21_236,
    unpriced_tokens: 0,
    estimated_cost: 0.034216,
    message: '',
    created_at: previewLogTimestamp(48_000),
    completed_at: previewLogTimestamp(43_714),
  },
  {
    id: 6,
    request_id: 'req-preview-retry',
    attempt_index: 1,
    account_id: 'oauth-team',
    account_name: '团队备用',
    account_type: 'oauth',
    source: 'proxy',
    method: 'POST',
    path: '/v1/responses',
    endpoint_family: 'responses',
    model: 'gpt-5.5',
    status: 'retry',
    http_status: 429,
    streaming: true,
    ttfb_ms: 238,
    duration_ms: 241,
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: 0,
    unpriced_tokens: 0,
    estimated_cost: 0,
    message: '上游限流，已切换下一个可用账号',
    created_at: previewLogTimestamp(49_000),
    completed_at: previewLogTimestamp(48_759),
  },
  {
    id: 5,
    request_id: 'req-preview-models',
    attempt_index: 1,
    account_id: 'oauth-pro',
    account_name: '日常开发',
    account_type: 'oauth',
    source: 'proxy',
    method: 'GET',
    path: '/v1/models',
    endpoint_family: 'models',
    model: '',
    status: 'success',
    http_status: 200,
    streaming: false,
    ttfb_ms: 184,
    duration_ms: 186,
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: 0,
    unpriced_tokens: 0,
    estimated_cost: 0,
    message: '',
    created_at: previewLogTimestamp(132_000),
    completed_at: previewLogTimestamp(131_814),
  },
  {
    id: 4,
    request_id: 'req-preview-error',
    attempt_index: 1,
    account_id: null,
    account_name: '',
    account_type: '',
    source: 'proxy',
    method: 'POST',
    path: '/v1/responses',
    endpoint_family: 'responses',
    model: 'gpt-5.6-terra',
    status: 'error',
    http_status: 503,
    streaming: false,
    ttfb_ms: null,
    duration_ms: 38,
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: 0,
    unpriced_tokens: 0,
    estimated_cost: 0,
    message: '没有可用且匹配该模型的上游',
    created_at: previewLogTimestamp(196_000),
    completed_at: previewLogTimestamp(195_962),
  },
  {
    id: 3,
    request_id: 'req-preview-cancelled',
    attempt_index: 1,
    account_id: 'oauth-pro',
    account_name: '日常开发',
    account_type: 'oauth',
    source: 'proxy',
    method: 'POST',
    path: '/v1/responses',
    endpoint_family: 'responses',
    model: 'gpt-5.4',
    status: 'cancelled',
    http_status: 200,
    streaming: true,
    ttfb_ms: 391,
    duration_ms: 1_842,
    input_tokens: 0,
    output_tokens: 0,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: 0,
    unpriced_tokens: 0,
    estimated_cost: 0,
    message: '客户端在流式响应完成前断开',
    created_at: previewLogTimestamp(310_000),
    completed_at: previewLogTimestamp(308_158),
  },
]

const wait = (duration: number) => new Promise((resolve) => window.setTimeout(resolve, duration))

function previewCodexTakeoverStatus(): CodexTakeoverStatus {
  const expectedBaseUrl = 'http://127.0.0.1:19090/v1'
  return {
    active: previewCodexTakeoverActive,
    backup_available: previewCodexTakeoverActive,
    codex_dir: 'C:\\Users\\demo\\.codex',
    auth_path: 'C:\\Users\\demo\\.codex\\auth.json',
    config_path: 'C:\\Users\\demo\\.codex\\config.toml',
    expected_base_url: expectedBaseUrl,
    configured_base_url: previewCodexTakeoverActive ? expectedBaseUrl : null,
    provider_id: previewCodexTakeoverActive ? 'custom' : null,
    model: 'gpt-5.5',
  }
}

function previewCodexSessionHistoryStatus(): CodexSessionHistoryStatus {
  return {
    active: previewCodexTakeoverActive,
    backup_available: previewCodexHistoryBackupAvailable,
    provider_id: 'custom',
    codex_dir: 'C:\\Users\\demo\\.codex',
    sessions_path: 'C:\\Users\\demo\\.codex\\sessions',
    archived_sessions_path: 'C:\\Users\\demo\\.codex\\archived_sessions',
    state_paths: ['C:\\Users\\demo\\.codex\\state_5.sqlite'],
  }
}

function previewQuota(accountId: string): AccountQuota {
  const account = previewAccounts.find((item) => item.id === accountId)!
  const fetchedAt = Date.now()
  return {
    user_id: account.chatgpt_user_id,
    account_id: account.chatgpt_account_id,
    email: account.email,
    plan_type: account.plan_type,
    fetched_at: fetchedAt,
    rate_limit_reset_credits: { available_count: accountId === 'oauth-pro' ? 2 : undefined },
    rate_limit: {
      allowed: true,
      limit_reached: false,
      primary_window: {
        used_percent: accountId === 'oauth-pro' ? 37.4 : 81,
        remaining_percent: accountId === 'oauth-pro' ? 62.6 : undefined,
        limit_window_seconds: 604800,
        reset_at: Math.floor((fetchedAt + 3 * 86_400_000) / 1000),
        num_requests: 527,
        num_tokens: 64_800_000,
      },
      secondary_window: {
        used_percent: accountId === 'oauth-pro' ? 12 : 44,
        remaining_percent: undefined,
        limit_window_seconds: 18000,
        reset_after_seconds: accountId === 'oauth-pro' ? 7800 : undefined,
        num_requests: 42,
        num_tokens: 5_200_000,
      },
    },
    additional_rate_limits: accountId === 'oauth-pro'
      ? [{
          limit_name: 'Codex',
          metered_feature: 'codex',
          rate_limit: {
            allowed: true,
            limit_reached: false,
            primary_window: { used_percent: 72, limit_window_seconds: 604800 },
            secondary_window: { remaining_percent: 91, limit_window_seconds: 18000, reset_after_seconds: 14200 },
          },
        }]
      : [],
  }
}

function previewRelayUsage(): RelayUsageSummary {
  return {
    today_actual_cost: 0,
    last_30_days_actual_cost: 15.3169,
    total_actual_cost: 15.3169,
    quota_used: 15.32,
    quota_limit: 25,
    balance: null,
    remaining: 9.68,
    plan: '团队套餐',
    mode: 'quota_limited',
    fetched_at: Date.now(),
  }
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!previewMode) return invoke<T>(command, args)
  await wait(command.startsWith('query_') ? 650 : 120)
  switch (command) {
    case 'get_proxy_info':
      return {
        port: 19090,
        proxy_profile: 'development',
        base_url: 'http://127.0.0.1:19090',
        access_token: previewAccessToken,
        running: true,
        account_count: previewAccounts.length,
        active_account_count: previewAccounts.filter((account) => account.status === 'active').length,
        total_requests: previewAccounts.reduce((sum, a) => sum + a.request_count, 0),
        input_tokens: 211340,
        output_tokens: 73410,
        cached_tokens: 62800,
        cache_write_tokens: 7420,
        reasoning_tokens: 18620,
        unpriced_tokens: 1280,
        total_tokens: 284750,
        total_cost: 0.004216,
        today_cost: 0.003184,
        pricing_updated_at: '2026-07-31',
        pricing_source: 'LiteLLM / Sub2API pricing snapshot',
        account_capacities: {
          'oauth-pro': 2,
          'oauth-team': 0,
          'api-key': 4,
          'oauth-disabled': 0,
        },
      } as T
    case 'list_accounts':
      return previewAccounts.map((account) => ({ ...account })) as T
    case 'import_accounts':
      return {
        total: (args?.contents as string[])?.length ?? 1,
        created: 2,
        updated: 1,
        failed: 1,
        errors: [{ index: 3, name: 'broken-upstream.json', message: '缺少可识别的 OAuth 或中转站凭据' }],
      } as T
    case 'inspect_clipboard_import': {
      const clipboardPreview = new URLSearchParams(window.location.search).get('clipboard')
      if (!clipboardPreview) return null as T
      return {
        candidate_id: 'preview-clipboard-account',
        source: clipboardPreview === 'cpa' ? 'cpa' : 'sub2api',
        detected_from: 'clipboard',
        account_count: 1,
        accounts: [{
          name: '开发账号',
          email: 'developer@example.com',
          account_type: 'oauth',
        }],
      } as T
    }
    case 'confirm_clipboard_import':
      return { total: 1, created: 1, updated: 0, failed: 0, errors: [] } as T
    case 'discard_clipboard_import':
      return true as T
    case 'list_trashed_accounts':
      return [] as T
    case 'restore_account':
      return true as T
    case 'purge_account':
      return true as T
    case 'purge_all_trashed':
      return 0 as T
    case 'refresh_account':
      return previewAccounts.find((account) => account.id === args?.id) as T
    case 'refresh_all_accounts':
      return { total: 2, created: 0, updated: 2, failed: 0, errors: [] } as T
    case 'test_account':
      return '连接正常，响应耗时 286ms' as T
    case 'open_relay_site':
      return 'https://relay.example/' as T
    case 'set_account_status': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (target) target.status = args?.status as AccountStatus
      return Boolean(target) as T
    }
    case 'set_account_priority': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (target) target.priority = args?.priority as number
      return Boolean(target) as T
    }
    case 'set_account_concurrency': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (target) target.concurrency = args?.concurrency as number
      return Boolean(target) as T
    }
    case 'delete_account': {
      const previousLength = previewAccounts.length
      previewAccounts = previewAccounts.filter((account) => account.id !== args?.id)
      return (previewAccounts.length !== previousLength) as T
    }
    case 'export_accounts':
      return JSON.stringify(previewAccounts, null, 2) as T
    case 'reset_request_counts': {
      const count = previewAccounts.filter((a) => a.request_count > 0).length
      previewAccounts.forEach((a) => { a.request_count = 0; a.last_used_at = null })
      return count as T
    }
    case 'reset_access_token':
      previewAccessToken = `sk-local-${crypto.randomUUID().replaceAll('-', '')}`
      return previewAccessToken as T
    case 'get_codex_takeover_status':
      return previewCodexTakeoverStatus() as T
    case 'set_codex_takeover':
      previewCodexTakeoverActive = Boolean(args?.enabled)
      return previewCodexTakeoverStatus() as T
    case 'get_codex_session_history_status':
      return previewCodexSessionHistoryStatus() as T
    case 'has_codex_session_history_backup':
      return previewCodexHistoryBackupAvailable as T
    case 'migrate_codex_session_history':
      if (!previewCodexTakeoverActive) {
        return {
          migrated_jsonl_files: 0,
          migrated_state_rows: 0,
          skipped_reason: 'not_unified',
        } as T
      }
      previewCodexHistoryBackupAvailable = true
      return {
        migrated_jsonl_files: 8,
        migrated_state_rows: 23,
        skipped_reason: null,
      } as T
    case 'restore_codex_session_history':
      if (!previewCodexHistoryBackupAvailable) {
        return {
          restored_jsonl_files: 0,
          restored_state_rows: 0,
          skipped_reason: 'no_backup_ledger',
        } as T
      }
      return {
        restored_jsonl_files: 6,
        restored_state_rows: 19,
        skipped_reason: null,
      } as T
    case 'get_cache':
      return window.localStorage.getItem(
        `${PREVIEW_CACHE_PREFIX}${String(args?.key ?? '')}`,
      ) as T
    case 'set_cache':
      window.localStorage.setItem(
        `${PREVIEW_CACHE_PREFIX}${String(args?.key ?? '')}`,
        String(args?.value ?? ''),
      )
      return undefined as T
    case 'merge_cache_entries': {
      const storageKey = `${PREVIEW_CACHE_PREFIX}${String(args?.key ?? '')}`
      const currentRaw = window.localStorage.getItem(storageKey)
      let current: Record<string, unknown> = {}
      try {
        const parsed = currentRaw ? JSON.parse(currentRaw) : {}
        if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) current = parsed
      } catch {
        current = {}
      }
      const entries = args?.entries
      if (entries && typeof entries === 'object' && !Array.isArray(entries)) {
        Object.assign(current, entries)
      }
      window.localStorage.setItem(storageKey, JSON.stringify(current))
      return undefined as T
    }
    case 'query_account_quota':
      if (args?.id === 'oauth-disabled') throw new Error('OAuth token 已失效，请刷新后重试')
      return previewQuota(String(args?.id)) as T
    case 'query_all_quotas':
      return previewAccounts
        .filter((account) => account.account_type === 'oauth' && account.status === 'active')
        .map((account) => ({ account_id: account.id, quota: previewQuota(account.id) })) as T
    case 'query_relay_usage':
      return previewRelayUsage() as T
    case 'list_request_logs': {
      const query = (args?.query ?? {}) as RequestLogQuery
      const normalizedSearch = query.search?.trim().toLocaleLowerCase() ?? ''
      const limit = Math.min(500, Math.max(1, Math.trunc(query.limit ?? 100)))
      const filtered = previewRequestLogs
        .filter((item) => !query.status || item.status === query.status)
        .filter((item) => !query.account_id || item.account_id === query.account_id)
        .filter((item) => !query.source || item.source === query.source)
        .filter((item) => !query.before_id || item.id < query.before_id)
        .filter((item) => {
          if (!normalizedSearch) return true
          return [item.request_id, item.account_name, item.model, item.path, item.message]
            .some((value) => value.toLocaleLowerCase().includes(normalizedSearch))
        })
        .sort((left, right) => right.id - left.id)
      const items = filtered.slice(0, limit)
      const hasMore = filtered.length > items.length
      return {
        items: items.map((item) => ({ ...item })),
        has_more: hasMore,
        next_before_id: hasMore ? items.at(-1)?.id ?? null : null,
      } as T
    }
    case 'clear_request_logs': {
      const deleted = previewRequestLogs.length
      previewRequestLogs = []
      return deleted as T
    }
    case 'get_app_version':
      return { version: '0.1.0-alpha.3', commit: 'dev', build_time: '2026-08-02' } as T
    default:
      throw new Error(`Unsupported preview command: ${command}`)
  }
}

export const getProxyInfo = () => call<ProxyInfo>('get_proxy_info')

export const listAccounts = () => call<Account[]>('list_accounts')

export const importAccounts = (contents: string[], defaultPriority = 1) =>
  call<ImportResult>('import_accounts', { contents, defaultPriority })

export const inspectClipboardImport = () =>
  call<ClipboardImportCandidate | null>('inspect_clipboard_import')

export const confirmClipboardImport = (candidateId: string, defaultPriority = 1) =>
  call<ImportResult>('confirm_clipboard_import', { candidateId, defaultPriority })

export const discardClipboardImport = (candidateId: string) =>
  call<boolean>('discard_clipboard_import', { candidateId })

export const refreshAccount = (id: string) =>
  call<Account>('refresh_account', { id })

export const refreshAllAccounts = () =>
  call<ImportResult>('refresh_all_accounts')

export const testAccount = (id: string) =>
  call<string>('test_account', { id })

export const openRelaySite = (id: string) =>
  call<string>('open_relay_site', { id })

export const setAccountStatus = (id: string, status: AccountStatus) =>
  call<boolean>('set_account_status', { id, status })

export const setAccountPriority = (id: string, priority: number) =>
  call<boolean>('set_account_priority', { id, priority })

export const setAccountConcurrency = (id: string, concurrency: number) =>
  call<boolean>('set_account_concurrency', { id, concurrency })

export const deleteAccount = (id: string) =>
  call<boolean>('delete_account', { id })

export const listTrashedAccounts = () => call<Account[]>('list_trashed_accounts')

export const restoreAccount = (id: string) =>
  call<boolean>('restore_account', { id })

export const purgeAccount = (id: string) =>
  call<boolean>('purge_account', { id })

export const purgeAllTrashed = () => call<number>('purge_all_trashed')

export const exportAccounts = () => call<string>('export_accounts')

export const resetRequestCounts = () => call<number>('reset_request_counts')

export const resetAccessToken = () => call<string>('reset_access_token')

export const getCodexTakeoverStatus = () =>
  call<CodexTakeoverStatus>('get_codex_takeover_status')

export const setCodexTakeover = (enabled: boolean) =>
  call<CodexTakeoverStatus>('set_codex_takeover', { enabled })

export const getCodexSessionHistoryStatus = () =>
  call<CodexSessionHistoryStatus>('get_codex_session_history_status')

export const hasCodexSessionHistoryBackup = () =>
  call<boolean>('has_codex_session_history_backup')

export const migrateCodexSessionHistory = () =>
  call<CodexSessionHistoryMigrationResult>('migrate_codex_session_history')

export const restoreCodexSessionHistory = () =>
  call<CodexSessionHistoryRestoreResult>('restore_codex_session_history')

export const queryAccountQuota = (id: string) =>
  call<AccountQuota>('query_account_quota', { id })

export const queryAllQuotas = () =>
  call<AccountQuotaResult[]>('query_all_quotas')

export const queryRelayUsage = (id: string) =>
  call<RelayUsageSummary>('query_relay_usage', { id })

export const listRequestLogs = (query: RequestLogQuery = {}) =>
  call<RequestLogPage>('list_request_logs', { query })

export const clearRequestLogs = () => call<number>('clear_request_logs')

export const getCache = (key: string) =>
  call<string | null>('get_cache', { key })

export const setCache = (key: string, value: string) =>
  call<void>('set_cache', { key, value })

export const mergeCacheEntries = (key: string, entries: Record<string, unknown>) =>
  call<void>('merge_cache_entries', { key, entries })

export const getAppVersion = () => call<AppVersion>('get_app_version')
