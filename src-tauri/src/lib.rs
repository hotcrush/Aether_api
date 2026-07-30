mod account_import;
mod capacity;
mod codex_history;
mod codex_takeover;
mod db;
mod dns;
mod oauth;
mod pricing;
mod proxy;
mod quota;
mod relay_usage;

use account_import::{ClipboardImportSource, ImportMessage, ImportResult, ParsedClipboardImport};
use capacity::CapacityRegistry;
use db::{Account, Db, NewAccount, UpsertAction};
use serde::Serialize;
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_shell::ShellExt;
use tracing::warn;

#[derive(Debug, Clone, Serialize)]
struct ClipboardAccountSummary {
    name: String,
    email: String,
    account_type: String,
}

#[derive(Debug, Clone, Serialize)]
struct ClipboardImportCandidate {
    candidate_id: String,
    source: ClipboardImportSource,
    account_count: usize,
    accounts: Vec<ClipboardAccountSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClipboardFingerprint {
    hash: u64,
    length: usize,
}

#[derive(Debug)]
struct PendingClipboardImport {
    candidate_id: String,
    accounts: Vec<NewAccount>,
}

#[derive(Debug, Default)]
struct ClipboardImportState {
    last_fingerprint: Option<ClipboardFingerprint>,
    pending: Option<PendingClipboardImport>,
}

impl ClipboardImportState {
    fn mark_fingerprint(&mut self, fingerprint: ClipboardFingerprint) -> bool {
        if self.last_fingerprint == Some(fingerprint) {
            return false;
        }
        self.last_fingerprint = Some(fingerprint);
        true
    }

    fn store_candidate(
        &mut self,
        fingerprint: ClipboardFingerprint,
        parsed: ParsedClipboardImport,
    ) -> ClipboardImportCandidate {
        self.last_fingerprint = Some(fingerprint);
        let candidate_id = uuid::Uuid::new_v4().to_string();
        let candidate = ClipboardImportCandidate {
            candidate_id: candidate_id.clone(),
            source: parsed.source,
            account_count: parsed.accounts.len(),
            accounts: parsed
                .accounts
                .iter()
                .take(5)
                .map(clipboard_account_summary)
                .collect(),
        };
        self.pending = Some(PendingClipboardImport {
            candidate_id,
            accounts: parsed.accounts,
        });
        candidate
    }

    fn take_candidate(&mut self, candidate_id: &str) -> Option<Vec<NewAccount>> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.candidate_id == candidate_id)
        {
            return self.pending.take().map(|pending| pending.accounts);
        }
        None
    }

    fn discard_candidate(&mut self, candidate_id: &str) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.candidate_id == candidate_id)
        {
            self.pending = None;
            return true;
        }
        false
    }
}

struct ClipboardReadGuard<'a>(&'a AtomicBool);

impl Drop for ClipboardReadGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct AppState {
    pub db: Arc<Db>,
    pub app_data_dir: PathBuf,
    pub client: reqwest::Client,
    pub proxy_port: u16,
    pub proxy_profile: &'static str,
    pub capacity: Arc<CapacityRegistry>,
    pub access_token: Arc<RwLock<String>>,
    pub proxy_running: Arc<AtomicBool>,
    clipboard_import: Mutex<ClipboardImportState>,
    clipboard_reading: AtomicBool,
}

#[tauri::command]
fn list_accounts(state: tauri::State<AppState>) -> Result<Vec<Account>, String> {
    state.db.list_accounts().map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_account(state: tauri::State<AppState>, id: String) -> Result<bool, String> {
    state
        .db
        .delete_account(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn list_trashed_accounts(state: tauri::State<AppState>) -> Result<Vec<Account>, String> {
    state.db.list_trashed_accounts().map_err(|error| error.to_string())
}

#[tauri::command]
fn get_cache(state: tauri::State<AppState>, key: String) -> Result<Option<String>, String> {
    state
        .db
        .get_setting(&key)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_cache(state: tauri::State<AppState>, key: String, value: String) -> Result<(), String> {
    state
        .db
        .set_setting(&key, &value)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn restore_account(state: tauri::State<AppState>, id: String) -> Result<bool, String> {
    state
        .db
        .restore_account(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn purge_account(state: tauri::State<AppState>, id: String) -> Result<bool, String> {
    state
        .db
        .purge_account(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn purge_all_trashed(state: tauri::State<AppState>) -> Result<u64, String> {
    state
        .db
        .purge_all_trashed()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_account_status(
    state: tauri::State<AppState>,
    id: String,
    status: String,
) -> Result<bool, String> {
    if !matches!(status.as_str(), "active" | "disabled") {
        return Err("账号状态无效".to_string());
    }
    state
        .db
        .set_status(&id, &status)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_account_priority(
    state: tauri::State<AppState>,
    id: String,
    priority: i64,
) -> Result<bool, String> {
    if !(0..=1000).contains(&priority) {
        return Err("优先级必须在 0 到 1000 之间".to_string());
    }
    state
        .db
        .set_priority(&id, priority)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_account_concurrency(
    state: tauri::State<AppState>,
    id: String,
    concurrency: i64,
) -> Result<bool, String> {
    if !(1..=1000).contains(&concurrency) {
        return Err("容量必须在 1 到 1000 之间".to_string());
    }
    state
        .db
        .set_concurrency(&id, concurrency)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_proxy_info(state: tauri::State<AppState>) -> serde_json::Value {
    let accounts = state.db.list_accounts().unwrap_or_default();
    let account_count = accounts.len();
    let active_account_count = accounts
        .iter()
        .filter(|account| account.status == "active")
        .count();
    let total_requests = state.db.total_request_count().unwrap_or(0);
    let usage = state.db.usage_totals().unwrap_or_default();
    let access_token = state
        .access_token
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    json!({
        "port": state.proxy_port,
        "proxy_profile": state.proxy_profile,
        "base_url": format!("http://127.0.0.1:{}", state.proxy_port),
        "access_token": access_token,
        "running": state.proxy_running.load(Ordering::Acquire),
        "account_count": account_count,
        "active_account_count": active_account_count,
        "total_requests": total_requests,
        "total_tokens": usage.total_tokens,
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cached_tokens": usage.cached_tokens,
        "cache_write_tokens": usage.cache_write_tokens,
        "reasoning_tokens": usage.reasoning_tokens,
        "unpriced_tokens": usage.unpriced_tokens,
        "total_cost": usage.total_cost,
        "pricing_updated_at": pricing::PRICING_UPDATED_AT,
        "pricing_source": pricing::PRICING_SOURCE,
        "account_capacities": state.capacity.snapshot(),
    })
}

const DEVELOPMENT_PROXY_PORT: u16 = 19_090;
const PRODUCTION_PROXY_PORT: u16 = 9_090;

fn proxy_profile() -> (&'static str, &'static str, u16) {
    if cfg!(debug_assertions) {
        ("development", "proxy_port_development", DEVELOPMENT_PROXY_PORT)
    } else {
        ("production", "proxy_port", PRODUCTION_PROXY_PORT)
    }
}

fn valid_proxy_port(value: &str) -> Option<u16> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port >= 1024)
}

fn configured_proxy_port(db: &Db) -> u16 {
    let (profile, setting_key, default_port) = proxy_profile();
    let profile_env = if profile == "development" {
        "AETHER_DEVELOPMENT_PROXY_PORT"
    } else {
        "AETHER_PRODUCTION_PROXY_PORT"
    };
    for key in [profile_env, "AETHER_PROXY_PORT"] {
        if let Ok(value) = std::env::var(key) {
            if let Some(port) = valid_proxy_port(&value) {
                return port;
            }
            warn!(environment = key, value, "忽略无效的代理端口环境变量");
        }
    }
    let stored = db
        .get_or_create_setting(setting_key, &default_port.to_string())
        .ok();
    if let Some(port) = stored.as_deref().and_then(valid_proxy_port) {
        return port;
    }
    let _ = db.set_setting(setting_key, &default_port.to_string());
    default_port
}

fn clipboard_fingerprint(content: &str) -> ClipboardFingerprint {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    ClipboardFingerprint {
        hash: hasher.finish(),
        length: content.len(),
    }
}

fn clipboard_account_summary(account: &NewAccount) -> ClipboardAccountSummary {
    let name = if !account.name.trim().is_empty() {
        account.name.trim().to_string()
    } else if !account.email.trim().is_empty() {
        account.email.trim().to_string()
    } else if account.account_type == "oauth" {
        "OpenAI OAuth".to_string()
    } else {
        "OpenAI 中转站".to_string()
    };
    ClipboardAccountSummary {
        name,
        email: account.email.trim().to_string(),
        account_type: account.account_type.clone(),
    }
}

fn validate_default_priority(default_priority: Option<i64>) -> Result<i64, String> {
    let default_priority = default_priority.unwrap_or(1);
    if !(0..=1000).contains(&default_priority) {
        return Err("默认优先级必须在 0 到 1000 之间".to_string());
    }
    Ok(default_priority)
}

async fn import_parsed_accounts(
    state: &AppState,
    accounts: Vec<NewAccount>,
    parse_errors: Vec<ImportMessage>,
    default_priority: i64,
) -> ImportResult {
    let mut result = ImportResult {
        total: accounts.len() + parse_errors.len(),
        failed: parse_errors.len(),
        errors: parse_errors,
        ..ImportResult::default()
    };
    let now = chrono::Utc::now().timestamp();

    for (offset, mut account) in accounts.into_iter().enumerate() {
        if account.priority.is_none() {
            account.priority = Some(default_priority);
        }
        let display_index = offset + 1;
        let display_name = account.name.clone();
        if account.account_type == "oauth" {
            let expired = account
                .expires_at
                .map(|expires| expires <= now - 120)
                .unwrap_or(false);
            if account.access_token.is_empty() || expired {
                if account.refresh_token.is_empty() {
                    result.failed += 1;
                    result.errors.push(ImportMessage {
                        index: display_index,
                        name: display_name,
                        message: if expired {
                            "access_token 已过期且没有 refresh_token".to_string()
                        } else {
                            "缺少 access_token".to_string()
                        },
                    });
                    continue;
                }
                match oauth::refresh_new_account(&state.client, &account).await {
                    Ok(refreshed) => account = refreshed,
                    Err(message) => {
                        result.failed += 1;
                        result.errors.push(ImportMessage {
                            index: display_index,
                            name: display_name,
                            message,
                        });
                        continue;
                    }
                }
            }
        } else if account.api_key.is_empty() {
            result.failed += 1;
            result.errors.push(ImportMessage {
                index: display_index,
                name: display_name,
                message: "缺少 api_key".to_string(),
            });
            continue;
        }

        match state.db.upsert_account(&account) {
            Ok((_, UpsertAction::Created)) => result.created += 1,
            Ok((_, UpsertAction::Updated)) => result.updated += 1,
            Err(error) => {
                result.failed += 1;
                result.errors.push(ImportMessage {
                    index: display_index,
                    name: display_name,
                    message: format!("保存账号失败: {error}"),
                });
            }
        }
    }
    result
}

#[tauri::command]
async fn inspect_clipboard_import(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<ClipboardImportCandidate>, String> {
    if state
        .clipboard_reading
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(None);
    }
    let _reading_guard = ClipboardReadGuard(&state.clipboard_reading);
    let clipboard_result =
        tauri::async_runtime::spawn_blocking(move || app.clipboard().read_text())
            .await
            .map_err(|error| format!("读取剪贴板任务失败: {error}"))?;
    let content = match clipboard_result {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };
    let fingerprint = clipboard_fingerprint(&content);

    {
        let mut clipboard_import = state
            .clipboard_import
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !clipboard_import.mark_fingerprint(fingerprint) {
            return Ok(None);
        }
    }

    let parsed = match account_import::parse_clipboard_import(&content) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(None),
    };
    let candidate = state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .store_candidate(fingerprint, parsed);
    Ok(Some(candidate))
}

#[tauri::command]
async fn confirm_clipboard_import(
    state: tauri::State<'_, AppState>,
    candidate_id: String,
    default_priority: Option<i64>,
) -> Result<ImportResult, String> {
    let default_priority = validate_default_priority(default_priority)?;
    let accounts = state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_candidate(&candidate_id)
        .ok_or_else(|| "剪贴板导入候选已失效".to_string())?;
    Ok(import_parsed_accounts(&state, accounts, Vec::new(), default_priority).await)
}

#[tauri::command]
fn discard_clipboard_import(state: tauri::State<AppState>, candidate_id: String) -> bool {
    state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .discard_candidate(&candidate_id)
}

#[tauri::command]
async fn import_accounts(
    state: tauri::State<'_, AppState>,
    contents: Vec<String>,
    default_priority: Option<i64>,
) -> Result<ImportResult, String> {
    if contents.is_empty() || contents.len() > 20 {
        return Err("请选择 1 到 20 个导入内容".to_string());
    }
    let default_priority = validate_default_priority(default_priority)?;
    let (accounts, parse_errors) = account_import::parse_import_contents(&contents);
    Ok(import_parsed_accounts(&state, accounts, parse_errors, default_priority).await)
}

#[tauri::command]
async fn refresh_account(state: tauri::State<'_, AppState>, id: String) -> Result<Account, String> {
    let account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;
    let refreshed = oauth::refresh_account(&state.client, &account).await;
    match refreshed {
        Ok(credentials) => state
            .db
            .update_oauth_tokens(&id, &credentials)
            .map_err(|error| format!("保存刷新结果失败: {error}")),
        Err(message) => {
            let _ = state.db.set_error(&id, &message);
            Err(message)
        }
    }
}

#[tauri::command]
async fn refresh_all_accounts(state: tauri::State<'_, AppState>) -> Result<ImportResult, String> {
    let accounts = state
        .db
        .list_accounts()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|account| account.account_type == "oauth" && account.status == "active")
        .collect::<Vec<_>>();
    let mut result = ImportResult {
        total: accounts.len(),
        ..ImportResult::default()
    };
    for (index, account) in accounts.into_iter().enumerate() {
        match oauth::refresh_account(&state.client, &account).await {
            Ok(credentials) => match state.db.update_oauth_tokens(&account.id, &credentials) {
                Ok(_) => result.updated += 1,
                Err(error) => {
                    result.failed += 1;
                    result.errors.push(ImportMessage {
                        index: index + 1,
                        name: account.name,
                        message: format!("保存刷新结果失败: {error}"),
                    });
                }
            },
            Err(message) => {
                let _ = state.db.set_error(&account.id, &message);
                result.failed += 1;
                result.errors.push(ImportMessage {
                    index: index + 1,
                    name: account.name,
                    message,
                });
            }
        }
    }
    Ok(result)
}

#[tauri::command]
async fn test_account(state: tauri::State<'_, AppState>, id: String) -> Result<String, String> {
    let mut account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;
    if account.account_type == "oauth"
        && (account.access_token.is_empty()
            || account
                .expires_at
                .map(|expires| expires <= chrono::Utc::now().timestamp() + 120)
                .unwrap_or(false))
        && !account.refresh_token.is_empty()
    {
        let refreshed = oauth::refresh_account(&state.client, &account).await?;
        account = state
            .db
            .update_oauth_tokens(&id, &refreshed)
            .map_err(|error| format!("保存刷新结果失败: {error}"))?;
    }

    let (url, token) = if account.account_type == "oauth" {
        (
            format!(
                "https://chatgpt.com/backend-api/codex/models?client_version={}",
                "0.144.1"
            ),
            account.access_token.as_str(),
        )
    } else {
        let base = if account.base_url.trim().is_empty() {
            "https://api.openai.com"
        } else {
            account.base_url.trim_end_matches('/')
        };
        let url = if base.ends_with("/v1") {
            format!("{base}/models")
        } else {
            format!("{base}/v1/models")
        };
        (url, account.api_key.as_str())
    };
    let mut request = state.client.get(url).bearer_auth(token);
    if account.account_type == "oauth" {
        request = request
            .header(
                "User-Agent",
                "codex_cli_rs/0.144.1 (Windows 11; x86_64) Windows_Terminal",
            )
            .header("originator", "codex_cli_rs")
            .header("version", "0.144.1");
        if !account.chatgpt_account_id.is_empty() {
            request = request.header("chatgpt-account-id", &account.chatgpt_account_id);
        }
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("连接失败: {error}"))?;
    if response.status().is_success() {
        let _ = state.db.set_error(&id, "");
        Ok(format!("连接正常 ({})", response.status()))
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = format!(
            "连接失败 ({status}): {}",
            body.chars().take(240).collect::<String>()
        );
        let _ = state.db.set_error(&id, &message);
        Err(message)
    }
}

async fn account_for_quota(
    state: &tauri::State<'_, AppState>,
    id: &str,
) -> Result<Account, String> {
    let mut account = state
        .db
        .get_account(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;
    if account.account_type != "oauth" {
        return Err("只有 OpenAI OAuth 账号支持额度查询".to_string());
    }

    let needs_refresh = account.access_token.is_empty()
        || account
            .expires_at
            .map(|expires| expires <= chrono::Utc::now().timestamp() + 120)
            .unwrap_or(false);
    if needs_refresh && !account.refresh_token.is_empty() {
        let refreshed = oauth::refresh_account(&state.client, &account).await?;
        account = state
            .db
            .update_oauth_tokens(id, &refreshed)
            .map_err(|error| format!("保存刷新结果失败: {error}"))?;
    }
    Ok(account)
}

#[tauri::command]
async fn query_account_quota(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<quota::QuotaUsage, String> {
    query_quota_for_id(&state, &id).await
}

async fn query_quota_for_id(
    state: &tauri::State<'_, AppState>,
    id: &str,
) -> Result<quota::QuotaUsage, String> {
    let account = account_for_quota(state, id).await?;
    match quota::query_usage(&state.client, &account).await {
        Ok(usage) => Ok(usage),
        Err(error) if error.status == Some(401) && !account.refresh_token.is_empty() => {
            let refreshed = oauth::refresh_account(&state.client, &account).await?;
            let refreshed = state
                .db
                .update_oauth_tokens(id, &refreshed)
                .map_err(|db_error| format!("保存刷新结果失败: {db_error}"))?;
            quota::query_usage(&state.client, &refreshed)
                .await
                .map_err(|retry_error| retry_error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
async fn query_all_quotas(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<quota::QuotaQueryResult>, String> {
    let account_ids = state
        .db
        .list_accounts()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|account| account.account_type == "oauth" && account.status == "active")
        .map(|account| account.id)
        .collect::<Vec<_>>();

    let mut results = Vec::with_capacity(account_ids.len());
    for account_id in account_ids {
        let result = match query_quota_for_id(&state, &account_id).await {
            Ok(usage) => quota::QuotaQueryResult {
                account_id,
                quota: Some(usage),
                error: None,
            },
            Err(error) => quota::QuotaQueryResult {
                account_id,
                quota: None,
                error: Some(error),
            },
        };
        results.push(result);
    }
    Ok(results)
}

#[tauri::command]
async fn query_relay_usage(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<relay_usage::RelayUsageSummary, String> {
    let account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "中转站不存在".to_string())?;
    relay_usage::query_usage(&state.client, &account).await
}

fn relay_site_url(base_url: &str) -> Result<reqwest::Url, String> {
    let raw = base_url.trim();
    if raw.is_empty() {
        return Err("中转站缺少 API 地址".to_string());
    }

    let mut url =
        reqwest::Url::parse(raw).map_err(|error| format!("中转站 API 地址无效: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("中转站 API 地址必须是有效的 HTTP(S) 地址".to_string());
    }

    url.set_username("")
        .map_err(|_| "中转站 API 地址无效".to_string())?;
    url.set_password(None)
        .map_err(|_| "中转站 API 地址无效".to_string())?;
    if let Some(site_host) = url
        .host_str()
        .and_then(|host| host.strip_prefix("api."))
        .map(str::to_owned)
    {
        url.set_host(Some(&site_host))
            .map_err(|_| "中转站 API 地址无效".to_string())?;
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

#[tauri::command]
fn open_relay_site(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    id: String,
) -> Result<(), String> {
    let account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "中转站不存在".to_string())?;
    if account.account_type != "api_key" {
        return Err("只有中转站账号支持打开网页".to_string());
    }

    let site_url = relay_site_url(&account.base_url)?;
    #[allow(deprecated)]
    app.shell()
        .open(site_url.as_str(), None)
        .map_err(|error| format!("无法打开中转站网页: {error}"))
}

#[tauri::command]
fn export_accounts(state: tauri::State<AppState>) -> Result<String, String> {
    let value = state.db.export_data().map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

#[tauri::command]
fn reset_request_counts(state: tauri::State<AppState>) -> Result<u64, String> {
    state
        .db
        .reset_request_counts()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn reset_access_token(state: tauri::State<AppState>) -> Result<String, String> {
    let access_token = format!("sk-local-{}", uuid::Uuid::new_v4().simple());
    let proxy_base_url = format!("http://127.0.0.1:{}/v1", state.proxy_port);
    codex_takeover::refresh_takeover_token_if_active(&state.db, &proxy_base_url, &access_token)?;
    state
        .db
        .set_setting("access_token", &access_token)
        .map_err(|error| error.to_string())?;
    *state
        .access_token
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = access_token.clone();
    Ok(access_token)
}

#[tauri::command]
fn get_codex_takeover_status(
    state: tauri::State<AppState>,
) -> Result<codex_takeover::CodexTakeoverStatus, String> {
    let proxy_base_url = format!("http://127.0.0.1:{}/v1", state.proxy_port);
    codex_takeover::takeover_status(&state.db, &proxy_base_url)
}

#[tauri::command]
fn set_codex_takeover(
    state: tauri::State<AppState>,
    enabled: bool,
) -> Result<codex_takeover::CodexTakeoverStatus, String> {
    let proxy_base_url = format!("http://127.0.0.1:{}/v1", state.proxy_port);
    if enabled {
        let access_token = state
            .access_token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        codex_takeover::enable_takeover(&state.db, &proxy_base_url, &access_token)
    } else {
        codex_takeover::disable_takeover(&state.db, &proxy_base_url)
    }
}

#[tauri::command]
fn get_codex_session_history_status(
    state: tauri::State<AppState>,
) -> Result<codex_history::CodexSessionHistoryStatus, String> {
    codex_history::session_history_status(&state.app_data_dir)
}

#[tauri::command]
fn has_codex_session_history_backup(state: tauri::State<AppState>) -> Result<bool, String> {
    codex_history::has_unify_history_backup(&state.app_data_dir)
}

#[tauri::command]
fn migrate_codex_session_history(
    state: tauri::State<AppState>,
) -> Result<codex_history::CodexSessionHistoryMigrationResult, String> {
    codex_history::migrate_existing_history(&state.app_data_dir)
}

#[tauri::command]
fn restore_codex_session_history(
    state: tauri::State<AppState>,
) -> Result<codex_history::CodexSessionHistoryRestoreResult, String> {
    codex_history::restore_official_history(&state.app_data_dir)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let app_dir = app.path().app_data_dir().expect("无法获取应用数据目录");
            std::fs::create_dir_all(&app_dir).ok();

            let db = Arc::new(Db::new(&app_dir.join("sub2api.db")).expect("初始化数据库失败"));
            let proxy_port = configured_proxy_port(&db);
            let (proxy_profile, _, _) = proxy_profile();
            let generated_token = format!("sk-local-{}", uuid::Uuid::new_v4().simple());
            let access_token = db
                .get_or_create_setting("access_token", &generated_token)
                .expect("初始化本地访问密钥失败");
            let access_token = Arc::new(RwLock::new(access_token));
            let proxy_running = Arc::new(AtomicBool::new(false));
            let capacity = Arc::new(CapacityRegistry::default());
            let client = dns::build_client(120, 15);

            let proxy_db = Arc::clone(&db);
            let proxy_token = Arc::clone(&access_token);
            let proxy_status = Arc::clone(&proxy_running);
            let proxy_capacity = Arc::clone(&capacity);
            tauri::async_runtime::spawn(async move {
                proxy::start_proxy_server(
                    proxy_db,
                    proxy_port,
                    proxy_token,
                    proxy_status,
                    proxy_capacity,
                )
                .await;
            });

            app.manage(AppState {
                db,
                app_data_dir: app_dir.clone(),
                client,
                proxy_port,
                proxy_profile,
                capacity,
                access_token,
                proxy_running,
                clipboard_import: Mutex::new(ClipboardImportState::default()),
                clipboard_reading: AtomicBool::new(false),
            });

            // System tray icon – minimize to tray on close
            let show_item = MenuItemBuilder::with_id("show", "显示窗口").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "退出 Aether").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&show_item)
                .item(&quit_item)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .tooltip("Aether")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            delete_account,
            list_trashed_accounts,
            restore_account,
            purge_account,
            purge_all_trashed,
            get_cache,
            set_cache,
            set_account_status,
            set_account_priority,
            set_account_concurrency,
            get_proxy_info,
            inspect_clipboard_import,
            confirm_clipboard_import,
            discard_clipboard_import,
            import_accounts,
            refresh_account,
            refresh_all_accounts,
            test_account,
            query_account_quota,
            query_all_quotas,
            query_relay_usage,
            open_relay_site,
            export_accounts,
            reset_request_counts,
            reset_access_token,
            get_codex_takeover_status,
            set_codex_takeover,
            get_codex_session_history_status,
            has_codex_session_history_backup,
            migrate_codex_session_history,
            restore_codex_session_history,
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用失败");
}

#[cfg(test)]
mod relay_site_tests {
    use super::relay_site_url;

    #[test]
    fn normalizes_relay_api_url_to_site_url() {
        let cases = [
            ("https://relay.example/v1", "https://relay.example/"),
            (
                "https://relay.example/v1/usage/?range=month#details",
                "https://relay.example/",
            ),
            (
                "https://relay.example/sub2api/v1/",
                "https://relay.example/",
            ),
            (
                "http://localhost:3000/gateway/?source=app#usage",
                "http://localhost:3000/",
            ),
            ("https://relay.example/api/v1", "https://relay.example/"),
            ("https://api.relay.example/v1", "https://relay.example/"),
            (
                "https://user:secret@relay.example/api/v1",
                "https://relay.example/",
            ),
        ];

        for (base_url, expected) in cases {
            assert_eq!(relay_site_url(base_url).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn rejects_missing_or_non_http_relay_url() {
        for base_url in ["", "/v1", "ftp://relay.example/v1"] {
            assert!(relay_site_url(base_url).is_err(), "accepted {base_url}");
        }
    }
}

#[cfg(test)]
mod clipboard_import_tests {
    use super::*;

    fn parsed_accounts(count: usize) -> ParsedClipboardImport {
        ParsedClipboardImport {
            source: ClipboardImportSource::Sub2api,
            accounts: (0..count)
                .map(|index| NewAccount {
                    name: format!("Account {index}"),
                    account_type: "oauth".to_string(),
                    access_token: format!("secret-access-{index}"),
                    refresh_token: format!("secret-refresh-{index}"),
                    email: format!("account-{index}@example.com"),
                    ..NewAccount::default()
                })
                .collect(),
        }
    }

    #[test]
    fn clipboard_candidate_is_sanitized_and_limited() {
        let mut state = ClipboardImportState::default();
        let fingerprint = clipboard_fingerprint("candidate-one");
        let candidate = state.store_candidate(fingerprint, parsed_accounts(7));
        let serialized = serde_json::to_string(&candidate).unwrap();

        assert_eq!(candidate.account_count, 7);
        assert_eq!(candidate.accounts.len(), 5);
        assert_eq!(
            serde_json::to_value(&candidate).unwrap()["source"],
            "sub2api"
        );
        assert!(!serialized.contains("secret-access"));
        assert!(!serialized.contains("secret-refresh"));
        assert!(state.pending.as_ref().is_some_and(|pending| {
            pending.accounts.len() == 7 && pending.accounts[0].access_token == "secret-access-0"
        }));
    }

    #[test]
    fn clipboard_candidate_dedupes_and_is_consumed_once() {
        let mut state = ClipboardImportState::default();
        let first = clipboard_fingerprint("candidate-one");
        let second = clipboard_fingerprint("candidate-two");

        assert!(state.mark_fingerprint(first));
        assert!(!state.mark_fingerprint(first));
        assert!(state.mark_fingerprint(second));

        let candidate = state.store_candidate(second, parsed_accounts(1));
        assert!(state.take_candidate("wrong-id").is_none());
        assert_eq!(
            state.take_candidate(&candidate.candidate_id).unwrap().len(),
            1
        );
        assert!(state.take_candidate(&candidate.candidate_id).is_none());
    }

    #[test]
    fn discard_only_clears_the_matching_candidate() {
        let mut state = ClipboardImportState::default();
        let fingerprint = clipboard_fingerprint("candidate-one");
        let candidate = state.store_candidate(fingerprint, parsed_accounts(1));

        assert!(!state.discard_candidate("wrong-id"));
        assert!(state.pending.is_some());
        assert!(state.discard_candidate(&candidate.candidate_id));
        assert!(state.pending.is_none());
    }
}
