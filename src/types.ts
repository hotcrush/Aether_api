export type AccountStatus = 'active' | 'disabled'
export type AccountTypeFilter = 'all' | 'oauth' | 'api_key'

export interface AppVersion {
  version: string
  commit: string
  build_time: string
  profile: string
  tauri_version: string
}

export interface Account {
  id: string
  name: string
  account_type: 'oauth' | 'api_key'
  credential_masked: string
  refreshable: boolean
  base_url: string
  chatgpt_account_id: string
  chatgpt_user_id: string
  email: string
  plan_type: string
  expires_at: number | null
  priority: number
  models: string[]
  weight: number
  concurrency: number
  rate_multiplier: number
  auto_sync_rate_multiplier: boolean
  status: AccountStatus
  last_error: string
  last_used_at: string | null
  request_count: number
  created_at: string
  updated_at: string
}

export interface AccountUpdate {
  name: string
  api_key: string | null
  base_url: string
  models: string[]
  priority: number
  weight: number
  concurrency: number
  rate_multiplier: number
}

export interface CostGuardSettings {
  enabled: boolean
  max_cost_multiplier: number
  safety_buffer: number
}

export interface OutboundProxySettings {
  enabled: boolean
  url: string
}

export interface OpenAIAuthorization {
  authUrl: string
  sessionId: string
  state: string
}

export interface ProxyInfo {
  port: number
  proxy_profile: 'development' | 'production'
  base_url: string
  access_token: string
  running: boolean
  account_count: number
  active_account_count: number
  total_requests: number
  input_tokens: number
  output_tokens: number
  cached_tokens: number
  cache_write_tokens: number
  reasoning_tokens: number
  unpriced_tokens: number
  total_tokens: number
  total_cost: number
  today_cost: number
  pricing_updated_at: string
  pricing_source: string
  account_capacities: Record<string, number>
}

export type RequestLogStatus = 'pending' | 'success' | 'retry' | 'error' | 'cancelled'

export interface RequestLog {
  id: number
  request_id: string
  attempt_index: number
  account_id: string | null
  account_name: string
  account_type: string
  source: string
  method: string
  path: string
  endpoint_family: string
  model: string
  status: RequestLogStatus
  http_status: number | null
  streaming: boolean
  ttfb_ms: number | null
  duration_ms: number | null
  input_tokens: number
  output_tokens: number
  cached_tokens: number
  cache_write_tokens: number
  reasoning_tokens: number
  total_tokens: number
  unpriced_tokens: number
  estimated_cost: number
  message: string
  created_at: string
  completed_at: string | null
}

export interface RequestLogQuery {
  status?: RequestLogStatus
  account_id?: string
  source?: string
  search?: string
  before_id?: number
  limit?: number
}

export interface RequestLogPage {
  items: RequestLog[]
  has_more: boolean
  next_before_id: number | null
}

export interface CodexTakeoverStatus {
  active: boolean
  backup_available: boolean
  codex_dir: string
  auth_path: string
  config_path: string
  expected_base_url: string
  configured_base_url: string | null
  provider_id: string | null
  model: string | null
}

export interface CodexSessionHistoryStatus {
  active: boolean
  backup_available: boolean
  provider_id: string
  codex_dir: string
  sessions_path: string
  archived_sessions_path: string
  state_paths: string[]
}

export interface CodexSessionHistoryMigrationResult {
  migrated_jsonl_files: number
  migrated_state_rows: number
  skipped_reason: string | null
}

export interface CodexSessionHistoryRestoreResult {
  restored_jsonl_files: number
  restored_state_rows: number
  skipped_reason: string | null
}

export interface ImportMessage {
  index: number
  name: string
  message: string
}

export interface ImportResult {
  total: number
  created: number
  updated: number
  failed: number
  errors: ImportMessage[]
}

export interface ClipboardImportAccount {
  name: string
  email: string
  account_type: 'oauth' | 'api_key'
}

export interface ClipboardImportCandidate {
  candidate_id: string
  source: 'cpa' | 'sub2api'
  detected_from?: 'clipboard' | 'download'
  file_name?: string
  account_count: number
  accounts: ClipboardImportAccount[]
}

export interface ToastItem {
  id: number
  message: string
  error: boolean
}

export interface QuotaWindow {
  used_percent?: number | null
  remaining_percent?: number | null
  limit_window_seconds?: number | null
  reset_after_seconds?: number | null
  reset_at?: number | null
  num_requests?: number | null
  num_requests_limit?: number | null
  num_tokens?: number | null
  num_tokens_limit?: number | null
}

export interface QuotaRateLimit {
  allowed?: boolean | null
  limit_reached?: boolean | null
  primary_window?: QuotaWindow | null
  secondary_window?: QuotaWindow | null
}

export interface AdditionalRateLimit {
  limit_name: string
  metered_feature: string
  rate_limit: QuotaRateLimit | null
}

export interface AccountQuota {
  user_id: string
  account_id: string
  email: string
  plan_type: string
  rate_limit: QuotaRateLimit | null
  additional_rate_limits?: AdditionalRateLimit[]
  fetched_at: number | string
  rate_limit_reset_credits?: {
    available_count?: number | null
  } | null
  estimated_limit_usd?: number | null
  estimated_limit_window?: '5h' | '7d' | string | null
  estimated_sample_cost_usd?: number | null
  estimated_sample_requests?: number | null
  estimated_sample_used_percent?: number | null
}

export interface AccountQuotaResult {
  account_id: string
  quota?: AccountQuota | null
  error?: string | null
}

export type QuotaQueryState =
  | { status: 'loading' }
  | { status: 'success'; quota: AccountQuota }
  | { status: 'error'; error: string }

export interface RelayUsageSummary {
  today_actual_cost: number | null
  last_30_days_actual_cost: number | null
  total_actual_cost: number | null
  quota_used: number | null
  quota_limit: number | null
  balance: number | null
  remaining: number | null
  plan: string | null
  mode: string
  fetched_at: number | string
  /** `quota` is New API's site-defined integer unit; generic relays use `usd`. */
  provider?: 'generic' | 'new_api' | string
  unit?: 'usd' | 'quota' | string
  quota_per_unit?: number | null
  unlimited_quota?: boolean
  expires_at?: number | null
  token_name?: string | null
  remote_request_count?: number | null
  remote_last_request_at?: number | null
  remote_last_model?: string | null
}

export type RelayUsageQueryState =
  | { status: 'loading' }
  | { status: 'success'; usage: RelayUsageSummary }
  | { status: 'error'; error: string }
