export type AccountStatus = 'active' | 'disabled'
export type AccountTypeFilter = 'all' | 'oauth' | 'api_key'

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
  status: AccountStatus
  last_error: string
  last_used_at: string | null
  request_count: number
  created_at: string
  updated_at: string
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
  pricing_updated_at: string
  pricing_source: string
  account_capacities: Record<string, number>
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
  allowed_amount?: number | null
  used_amount?: number | null
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
  additional_rate_limits: AdditionalRateLimit[]
  fetched_at: number | string
  rate_limit_reset_credits?: {
    available_count?: number | null
  } | null
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
}

export type RelayUsageQueryState =
  | { status: 'loading' }
  | { status: 'success'; usage: RelayUsageSummary }
  | { status: 'error'; error: string }
