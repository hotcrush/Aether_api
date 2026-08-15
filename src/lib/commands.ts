import { invoke } from '@tauri-apps/api/core'
import type {
  Account,
  AccountUpdate,
  AccountQuota,
  AccountQuotaResult,
  AccountStatus,
  AppVersion,
  CodexSessionHistoryMigrationResult,
  CodexSessionHistoryRestoreResult,
  CodexSessionHistoryStatus,
  CodexPromptState,
  CodexSkillState,
  CodexTakeoverStatus,
  CodexClientSettings,
  CodexFingerprintSettings,
  ClipboardImportCandidate,
  CostGuardSettings,
  ImageGenerationSettings,
  ImportResult,
  OpenAIAuthorization,
  OutboundProxySettings,
  PickupOrderRecord,
  PickupOverview,
  PickupSettings,
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
    rate_multiplier: 1,
    auto_sync_rate_multiplier: false,
    locked: false,
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
    rate_multiplier: 1,
    auto_sync_rate_multiplier: false,
    locked: false,
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
    rate_multiplier: 1.2,
    auto_sync_rate_multiplier: true,
    locked: false,
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
    rate_multiplier: 1,
    auto_sync_rate_multiplier: false,
    locked: false,
    status: 'disabled',
    last_error: '上次测试：OAuth token 已失效',
    last_used_at: null,
    request_count: 18,
    created_at: '2026-06-01T10:00:00Z',
    updated_at: '2026-07-20T10:00:00Z',
  },
]

let previewAccessToken = 'sk-local-cf4456e6195e4461957af12029f7cdfb'
let previewPickupSettings: PickupSettings = { customer_token: 'cfk-preview-token' }
let previewPickupOrders: PickupOrderRecord[] = []
let previewCodexTakeoverActive = false
let previewCodexHistoryBackupAvailable = false
let previewCodexPromptState: CodexPromptState = {
  prompts: [{
    id: 'prompt-preview-review',
    name: '代码审查',
    content: '# 工作方式\n\n优先发现行为回归、安全问题和缺失测试。',
    updated_at: new Date().toISOString(),
  }],
  active_id: 'prompt-preview-review',
  file_path: 'C:\\Users\\demo\\.codex\\AGENTS.md',
  file_exists: true,
  current_content: '# 工作方式\n\n优先发现行为回归、安全问题和缺失测试。',
}
let previewCodexSkillState: CodexSkillState = {
  skills: [
    {
      directory: 'openai-docs',
      name: 'OpenAI Docs',
      description: '查询 OpenAI 与 Codex 官方文档。',
      enabled: true,
      path: 'C:\\Users\\demo\\.codex\\skills\\openai-docs',
    },
    {
      directory: 'release-helper',
      name: 'Release Helper',
      description: '生成发布说明并检查版本信息。',
      enabled: false,
      path: 'C:\\Users\\demo\\.codex\\.aether-disabled-skills\\release-helper',
    },
  ],
  skills_dir: 'C:\\Users\\demo\\.codex\\skills',
  disabled_dir: 'C:\\Users\\demo\\.codex\\.aether-disabled-skills',
}

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
    http_status: 101,
    streaming: true,
    transport: 'websocket',
    outbound_proxy: 'socks5h',
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
    upstream_response_model: 'gpt-5.6-sol',
    model_mismatch: false,
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
    transport: 'sse',
    outbound_proxy: 'http',
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
    upstream_response_model: 'gpt-5.4',
    model_mismatch: true,
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
    transport: 'sse',
    outbound_proxy: 'http',
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
    upstream_response_model: null,
    model_mismatch: null,
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
    transport: 'http',
    outbound_proxy: 'direct',
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
    upstream_response_model: null,
    model_mismatch: null,
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
    transport: 'http',
    outbound_proxy: 'direct',
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
    upstream_response_model: null,
    model_mismatch: null,
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
    transport: 'sse',
    outbound_proxy: 'socks5h',
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
    upstream_response_model: 'gpt-5.4',
    model_mismatch: false,
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
    local_window_usage: [
      {
        window: '5h',
        requests: accountId === 'oauth-pro' ? 83 : 41,
        tokens: accountId === 'oauth-pro' ? 8_320_000 : 3_410_000,
        api_equivalent_cost_usd: accountId === 'oauth-pro' ? 6.42 : 3.71,
      },
      {
        window: '7d',
        requests: accountId === 'oauth-pro' ? 527 : 213,
        tokens: accountId === 'oauth-pro' ? 64_800_000 : 21_300_000,
        api_equivalent_cost_usd: accountId === 'oauth-pro' ? 51.84 : 49.73,
      },
    ],
    rate_limit_reset_credits: accountId === 'oauth-pro'
      ? {
          available_count: 2,
          credits: [
            { expires_at: new Date(Date.now() + 5 * 86_400_000).toISOString() },
            { expires_at: new Date(Date.now() + 12 * 86_400_000).toISOString() },
          ],
        }
      : { available_count: 0, credits: [] },
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
    provider: 'generic',
    unit: 'usd',
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
    case 'sync_oauth_account_models': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (!target) throw new Error('账号不存在')
      target.models = ['gpt-5.6-sol', 'gpt-5.6-luna']
      return { ...target } as T
    }
    case 'open_relay_site':
      return 'https://relay.example/' as T
    case 'set_account_status': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (target) target.status = args?.status as AccountStatus
      return Boolean(target) as T
    }
    case 'set_account_locked': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (target) target.locked = Boolean(args?.locked)
      return Boolean(target) as T
    }
    case 'update_account': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      const update = args?.update as AccountUpdate | undefined
      if (!target || !update) throw new Error('上游不存在')
      target.name = update.name
      target.base_url = target.account_type === 'api_key' ? update.base_url : target.base_url
      target.models = target.account_type === 'api_key' ? [...update.models] : target.models
      target.priority = update.priority
      target.weight = update.weight
      target.concurrency = update.concurrency
      if (!target.auto_sync_rate_multiplier) target.rate_multiplier = update.rate_multiplier
      target.updated_at = new Date().toISOString()
      return { ...target } as T
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
    case 'set_account_rate_multiplier': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (target) target.rate_multiplier = args?.multiplier as number
      return Boolean(target) as T
    }
    case 'set_account_auto_sync_rate_multiplier': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (target) target.auto_sync_rate_multiplier = Boolean(args?.enabled)
      return Boolean(target) as T
    }
    case 'sync_account_rate_multiplier': {
      const target = previewAccounts.find((account) => account.id === args?.id)
      if (!target?.auto_sync_rate_multiplier) throw new Error('请先开启该中转站的自动倍率同步')
      target.rate_multiplier = 1.2
      return target.rate_multiplier as T
    }
    case 'begin_openai_oauth':
      return {
        authUrl: 'https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann',
        sessionId: 'preview-openai-oauth',
        state: 'preview-openai-oauth-state',
      } as T
    case 'get_cost_guard_settings':
      return JSON.parse(window.localStorage.getItem(`${PREVIEW_CACHE_PREFIX}cost-guard`) ?? '{"enabled":false,"max_cost_multiplier":1,"safety_buffer":0}') as T
    case 'update_cost_guard_settings':
      window.localStorage.setItem(`${PREVIEW_CACHE_PREFIX}cost-guard`, JSON.stringify(args?.settings))
      return args?.settings as T
    case 'get_outbound_proxy_settings':
      return JSON.parse(window.localStorage.getItem(`${PREVIEW_CACHE_PREFIX}outbound-proxy`) ?? '{"enabled":false,"url":"http://127.0.0.1:7890"}') as T
    case 'update_outbound_proxy_settings':
      window.localStorage.setItem(`${PREVIEW_CACHE_PREFIX}outbound-proxy`, JSON.stringify(args?.settings))
      return args?.settings as T
    case 'get_image_generation_settings':
      return JSON.parse(window.localStorage.getItem(`${PREVIEW_CACHE_PREFIX}image-generation`) ?? '{"enabled":false,"base_url":"https://api.openai.com/v1","api_key":""}') as T
    case 'update_image_generation_settings':
      window.localStorage.setItem(`${PREVIEW_CACHE_PREFIX}image-generation`, JSON.stringify(args?.settings))
      return args?.settings as T
    case 'get_pickup_settings':
      return { ...previewPickupSettings } as T
    case 'update_pickup_settings':
      previewPickupSettings = { ...(args?.settings as PickupSettings) }
      return { ...previewPickupSettings } as T
    case 'get_pickup_overview':
      return {
        balance: { balance_fen: 568, held_fen: 0, available_fen: 568, currency: 'CNY' },
        inventory: {
          product: 'team_1h',
          quantity: Number(args?.quantity ?? 1),
          available: 86,
          estimated_unit_price_fen: 360,
          estimated_total_fen: Number(args?.quantity ?? 1) * 360,
          hold_total_fen: Number(args?.quantity ?? 1) * 360,
        },
      } as T
    case 'list_pickup_orders':
      return previewPickupOrders.map((order) => ({ ...order })) as T
    case 'create_pickup_order': {
      const key = String(args?.idempotencyKey ?? '')
      const existing = previewPickupOrders.find((order) => order.idempotency_key === key)
      if (existing) return { ...existing } as T
      const now = new Date().toISOString()
      const order: PickupOrderRecord = {
        idempotency_key: key,
        order_id: `preview-${crypto.randomUUID().slice(0, 8)}`,
        product: 'team_1h',
        quantity: Number(args?.quantity ?? 1),
        state: 'completed',
        hold_total_fen: Number(args?.quantity ?? 1) * 360,
        charged_fen: Number(args?.quantity ?? 1) * 360,
        created_at: now,
        updated_at: now,
        response: {},
        import_attempted_at: now,
        import_result: { total: Number(args?.quantity ?? 1), created: Number(args?.quantity ?? 1), updated: 0, failed: 0, errors: [] },
        import_error: '',
        last_error: '',
      }
      previewPickupOrders = [order, ...previewPickupOrders].slice(0, 8)
      return { ...order } as T
    }
    case 'refresh_pickup_order': {
      const target = previewPickupOrders.find((order) => order.order_id === args?.orderId)
      if (!target) throw new Error('本地订单记录不存在')
      return { ...target } as T
    }
    case 'retry_pickup_order_import': {
      const target = previewPickupOrders.find((order) => order.order_id === args?.orderId)
      if (!target) throw new Error('本地订单记录不存在')
      return { ...target } as T
    }
    case 'get_codex_client_settings':
      return JSON.parse(window.localStorage.getItem(`${PREVIEW_CACHE_PREFIX}codex-client`) ?? '{"auto_sync_enabled":true,"effective_version":"0.147.0","synced_at":1786032000}') as T
    case 'update_codex_client_settings': {
      const current = JSON.parse(window.localStorage.getItem(`${PREVIEW_CACHE_PREFIX}codex-client`) ?? '{"auto_sync_enabled":true,"effective_version":"0.147.0","synced_at":1786032000}') as CodexClientSettings
      const next = { ...current, auto_sync_enabled: Boolean((args?.settings as { auto_sync_enabled?: boolean })?.auto_sync_enabled) }
      window.localStorage.setItem(`${PREVIEW_CACHE_PREFIX}codex-client`, JSON.stringify(next))
      return next as T
    }
    case 'sync_codex_client_version': {
      const current = JSON.parse(window.localStorage.getItem(`${PREVIEW_CACHE_PREFIX}codex-client`) ?? '{"auto_sync_enabled":true,"effective_version":"0.147.0","synced_at":1786032000}') as CodexClientSettings
      const next = { ...current, effective_version: '0.147.0', synced_at: Math.floor(Date.now() / 1000) }
      window.localStorage.setItem(`${PREVIEW_CACHE_PREFIX}codex-client`, JSON.stringify(next))
      return next as T
    }
    case 'get_codex_fingerprint_settings':
      return JSON.parse(window.localStorage.getItem(`${PREVIEW_CACHE_PREFIX}codex-fingerprint`) ?? '{"mode":"session"}') as T
    case 'update_codex_fingerprint_settings':
      window.localStorage.setItem(`${PREVIEW_CACHE_PREFIX}codex-fingerprint`, JSON.stringify(args?.settings))
      return args?.settings as T
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
    case 'get_codex_prompt_state':
      return {
        ...previewCodexPromptState,
        prompts: previewCodexPromptState.prompts.map((item) => ({ ...item })),
      } as T
    case 'save_codex_prompt': {
      const id = typeof args?.id === 'string' && args.id
        ? args.id
        : crypto.randomUUID().replaceAll('-', '')
      const preset = {
        id,
        name: String(args?.name ?? ''),
        content: String(args?.content ?? ''),
        updated_at: new Date().toISOString(),
      }
      const index = previewCodexPromptState.prompts.findIndex((item) => item.id === id)
      if (index >= 0) previewCodexPromptState.prompts[index] = preset
      else previewCodexPromptState.prompts.unshift(preset)
      if (Boolean(args?.activate) || previewCodexPromptState.active_id === id) {
        previewCodexPromptState.active_id = id
        previewCodexPromptState.current_content = preset.content
        previewCodexPromptState.file_exists = true
      }
      return { ...previewCodexPromptState, prompts: previewCodexPromptState.prompts.map((item) => ({ ...item })) } as T
    }
    case 'activate_codex_prompt': {
      const id = String(args?.id ?? '')
      const preset = previewCodexPromptState.prompts.find((item) => item.id === id)
      if (!preset) throw new Error('提示词预设不存在')
      previewCodexPromptState.active_id = id
      previewCodexPromptState.current_content = preset.content
      previewCodexPromptState.file_exists = true
      return { ...previewCodexPromptState, prompts: previewCodexPromptState.prompts.map((item) => ({ ...item })) } as T
    }
    case 'import_current_codex_prompt': {
      const id = crypto.randomUUID().replaceAll('-', '')
      previewCodexPromptState.prompts.unshift({
        id,
        name: '当前 AGENTS.md',
        content: previewCodexPromptState.current_content,
        updated_at: new Date().toISOString(),
      })
      previewCodexPromptState.active_id = id
      return { ...previewCodexPromptState, prompts: previewCodexPromptState.prompts.map((item) => ({ ...item })) } as T
    }
    case 'delete_codex_prompt': {
      const id = String(args?.id ?? '')
      previewCodexPromptState.prompts = previewCodexPromptState.prompts.filter((item) => item.id !== id)
      if (previewCodexPromptState.active_id === id) previewCodexPromptState.active_id = null
      return { ...previewCodexPromptState, prompts: previewCodexPromptState.prompts.map((item) => ({ ...item })) } as T
    }
    case 'get_codex_skill_state':
      return { ...previewCodexSkillState, skills: previewCodexSkillState.skills.map((item) => ({ ...item })) } as T
    case 'set_codex_skill_enabled': {
      const directory = String(args?.directory ?? '')
      previewCodexSkillState.skills = previewCodexSkillState.skills.map((item) => item.directory === directory
        ? {
            ...item,
            enabled: Boolean(args?.enabled),
            path: Boolean(args?.enabled)
              ? `${previewCodexSkillState.skills_dir}\\${item.directory}`
              : `${previewCodexSkillState.disabled_dir}\\${item.directory}`,
          }
        : item)
      return { ...previewCodexSkillState, skills: previewCodexSkillState.skills.map((item) => ({ ...item })) } as T
    }
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
        .filter((item) => !query.model_mismatch_only || item.model_mismatch === true)
        .filter((item) => !query.before_id || item.id < query.before_id)
        .filter((item) => {
          if (!normalizedSearch) return true
          return [
            item.request_id,
            item.account_name,
            item.model,
            item.upstream_response_model ?? '',
            item.path,
            item.message,
          ].some((value) => value.toLocaleLowerCase().includes(normalizedSearch))
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
      return { version: '0.1.0-alpha.21', commit: 'dev', build_time: '2026-08-15' } as T
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

export const syncOAuthAccountModels = (id: string) =>
  call<Account>('sync_oauth_account_models', { id })

export const openRelaySite = (id: string) =>
  call<string>('open_relay_site', { id })

export const setAccountStatus = (id: string, status: AccountStatus) =>
  call<boolean>('set_account_status', { id, status })

export const setAccountLocked = (id: string, locked: boolean) =>
  call<boolean>('set_account_locked', { id, locked })

export const updateAccount = (id: string, update: AccountUpdate) =>
  call<Account>('update_account', { id, update })

export const setAccountPriority = (id: string, priority: number) =>
  call<boolean>('set_account_priority', { id, priority })

export const setAccountConcurrency = (id: string, concurrency: number) =>
  call<boolean>('set_account_concurrency', { id, concurrency })

export const setAccountRateMultiplier = (id: string, multiplier: number) =>
  call<boolean>('set_account_rate_multiplier', { id, multiplier })

export const setAccountAutoSyncRateMultiplier = (id: string, enabled: boolean) =>
  call<boolean>('set_account_auto_sync_rate_multiplier', { id, enabled })

export const syncAccountRateMultiplier = (id: string) =>
  call<number>('sync_account_rate_multiplier', { id })

export const beginOpenAIOAuth = (name: string, priority: number) =>
  call<OpenAIAuthorization>('begin_openai_oauth', { name, priority })

export const getCostGuardSettings = () => call<CostGuardSettings>('get_cost_guard_settings')

export const updateCostGuardSettings = (settings: CostGuardSettings) =>
  call<CostGuardSettings>('update_cost_guard_settings', { settings })

export const getOutboundProxySettings = () => call<OutboundProxySettings>('get_outbound_proxy_settings')

export const updateOutboundProxySettings = (settings: OutboundProxySettings) =>
  call<OutboundProxySettings>('update_outbound_proxy_settings', { settings })

export const getImageGenerationSettings = () =>
  call<ImageGenerationSettings>('get_image_generation_settings')

export const updateImageGenerationSettings = (settings: ImageGenerationSettings) =>
  call<ImageGenerationSettings>('update_image_generation_settings', { settings })

export const getPickupSettings = () => call<PickupSettings>('get_pickup_settings')

export const updatePickupSettings = (settings: PickupSettings) =>
  call<PickupSettings>('update_pickup_settings', { settings })

export const getPickupOverview = (quantity: number) =>
  call<PickupOverview>('get_pickup_overview', { quantity })

export const listPickupOrders = () => call<PickupOrderRecord[]>('list_pickup_orders')

export const createPickupOrder = (quantity: number, idempotencyKey: string) =>
  call<PickupOrderRecord>('create_pickup_order', { quantity, idempotencyKey })

export const refreshPickupOrder = (orderId: string) =>
  call<PickupOrderRecord>('refresh_pickup_order', { orderId })

export const retryPickupOrderImport = (orderId: string) =>
  call<PickupOrderRecord>('retry_pickup_order_import', { orderId })

export const getCodexClientSettings = () =>
  call<CodexClientSettings>('get_codex_client_settings')

export const updateCodexClientSettings = (autoSyncEnabled: boolean) =>
  call<CodexClientSettings>('update_codex_client_settings', {
    settings: { auto_sync_enabled: autoSyncEnabled },
  })

export const syncCodexClientVersion = () =>
  call<CodexClientSettings>('sync_codex_client_version')

export const getCodexFingerprintSettings = () =>
  call<CodexFingerprintSettings>('get_codex_fingerprint_settings')

export const updateCodexFingerprintSettings = (settings: CodexFingerprintSettings) =>
  call<CodexFingerprintSettings>('update_codex_fingerprint_settings', { settings })

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

export const getCodexPromptState = () =>
  call<CodexPromptState>('get_codex_prompt_state')

export const saveCodexPrompt = (
  id: string | null,
  name: string,
  content: string,
  activate = false,
) => call<CodexPromptState>('save_codex_prompt', { id, name, content, activate })

export const activateCodexPrompt = (id: string) =>
  call<CodexPromptState>('activate_codex_prompt', { id })

export const importCurrentCodexPrompt = () =>
  call<CodexPromptState>('import_current_codex_prompt')

export const deleteCodexPrompt = (id: string) =>
  call<CodexPromptState>('delete_codex_prompt', { id })

export const getCodexSkillState = () =>
  call<CodexSkillState>('get_codex_skill_state')

export const setCodexSkillEnabled = (directory: string, enabled: boolean) =>
  call<CodexSkillState>('set_codex_skill_enabled', { directory, enabled })

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
