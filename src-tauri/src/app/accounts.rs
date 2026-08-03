use super::AppState;
use crate::account_import::{ImportMessage, ImportResult};
use crate::db::{normalize_models, Account, AccountUpdate, NewAccount, UpsertAction};
use crate::{logger, oauth};
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, EventTarget, Manager};
use tracing::{info, warn};

#[tauri::command]
pub(crate) fn list_accounts(state: tauri::State<AppState>) -> Result<Vec<Account>, String> {
    state.db.list_accounts().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn delete_account(state: tauri::State<AppState>, id: String) -> Result<bool, String> {
    state
        .db
        .delete_account(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn list_trashed_accounts(state: tauri::State<AppState>) -> Result<Vec<Account>, String> {
    state
        .db
        .list_trashed_accounts()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn restore_account(state: tauri::State<AppState>, id: String) -> Result<bool, String> {
    state
        .db
        .restore_account(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn purge_account(state: tauri::State<AppState>, id: String) -> Result<bool, String> {
    state
        .db
        .purge_account(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn purge_all_trashed(state: tauri::State<AppState>) -> Result<u64, String> {
    state
        .db
        .purge_all_trashed()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn set_account_status(
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

#[derive(Debug, Deserialize)]
pub(crate) struct AccountUpdatePayload {
    name: String,
    api_key: Option<String>,
    base_url: String,
    models: Vec<String>,
    priority: i64,
    weight: i64,
    concurrency: i64,
    rate_multiplier: f64,
}

#[tauri::command]
pub(crate) fn update_account(
    state: tauri::State<AppState>,
    id: String,
    update: AccountUpdatePayload,
) -> Result<Account, String> {
    let name = update.name.trim();
    if name.is_empty() {
        return Err("请输入账号名称".to_string());
    }
    if name.chars().count() > 100 {
        return Err("账号名称不能超过 100 个字符".to_string());
    }
    if !(0..=1000).contains(&update.priority) {
        return Err("优先级必须在 0 到 1000 之间".to_string());
    }
    if !(1..=1000).contains(&update.weight) {
        return Err("权重必须在 1 到 1000 之间".to_string());
    }
    if !(1..=1000).contains(&update.concurrency) {
        return Err("容量必须在 1 到 1000 之间".to_string());
    }
    if !update.rate_multiplier.is_finite() || !(0.0..=100.0).contains(&update.rate_multiplier) {
        return Err("成本倍率必须在 0 到 100 之间".to_string());
    }
    let current = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "上游不存在".to_string())?;
    if current.account_type == "api_key" {
        if update
            .api_key
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err("新 API Key 不能为空；留空表示保持原 Key".to_string());
        }
        let base_url = update.base_url.trim();
        if !base_url.is_empty() {
            let url =
                reqwest::Url::parse(base_url).map_err(|error| format!("Base URL 无效: {error}"))?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err("Base URL 必须是有效的 HTTP(S) 地址".to_string());
            }
        }
    }
    state
        .db
        .update_account(
            &id,
            &AccountUpdate {
                name: name.to_string(),
                api_key: update.api_key.map(|value| value.trim().to_string()),
                base_url: update.base_url.trim().to_string(),
                models: normalize_models(update.models),
                priority: update.priority,
                weight: update.weight,
                concurrency: update.concurrency,
                rate_multiplier: update.rate_multiplier,
            },
        )
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "上游不存在".to_string())
}

#[tauri::command]
pub(crate) fn set_account_priority(
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
pub(crate) fn set_account_concurrency(
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
pub(crate) fn set_account_rate_multiplier(
    state: tauri::State<AppState>,
    id: String,
    multiplier: f64,
) -> Result<bool, String> {
    if !multiplier.is_finite() || !(0.0..=100.0).contains(&multiplier) {
        return Err("成本倍率必须在 0 到 100 之间".to_string());
    }
    let account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "上游不存在".to_string())?;
    if account.auto_sync_rate_multiplier {
        return Err("已开启自动倍率同步，请先关闭后再手动修改".to_string());
    }
    state
        .db
        .set_rate_multiplier(&id, multiplier)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn set_account_auto_sync_rate_multiplier(
    state: tauri::State<AppState>,
    id: String,
    enabled: bool,
) -> Result<bool, String> {
    state
        .db
        .set_auto_sync_rate_multiplier(&id, enabled)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn sync_account_rate_multiplier(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<f64, String> {
    let client = state.client.load_full();
    crate::billing_sync::sync_account_rate_multiplier(&state.db, &client, &id).await
}

#[tauri::command]
pub(crate) fn begin_openai_oauth(
    state: tauri::State<AppState>,
    name: String,
    priority: Option<i64>,
) -> Result<crate::oauth::OpenAIAuthorization, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("请输入账号名称".to_string());
    }
    let priority = priority.unwrap_or(1);
    if !(0..=1000).contains(&priority) {
        return Err("优先级必须在 0 到 1000 之间".to_string());
    }
    if !state.openai_callback_ready.load(Ordering::Acquire) {
        return Err("无法监听 OpenAI 授权回调端口 1455，请关闭占用该端口的程序后重试".to_string());
    }
    state.oauth_sessions.begin(name.to_string(), priority)
}

#[derive(Deserialize)]
struct OpenAICallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Clone)]
pub(crate) struct OpenAICallbackServerState {
    app_handle: tauri::AppHandle,
}

pub(crate) async fn start_openai_callback_server(
    app_handle: tauri::AppHandle,
    ready: Arc<AtomicBool>,
) {
    let state = OpenAICallbackServerState { app_handle };
    let router = Router::new()
        .route("/auth/callback", get(handle_openai_callback))
        .with_state(state);
    let mut started = false;
    for address in ["127.0.0.1:1455", "[::1]:1455"] {
        match tokio::net::TcpListener::bind(address).await {
            Ok(listener) => {
                started = true;
                let router = router.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = axum::serve(listener, router).await {
                        warn!(%error, "OpenAI 授权回调服务已停止");
                    }
                });
            }
            Err(error) => warn!(%error, %address, "无法监听 OpenAI 授权回调地址"),
        }
    }
    if started {
        ready.store(true, Ordering::Release);
        info!("OpenAI 授权回调已监听: http://localhost:1455/auth/callback");
    }
}

async fn handle_openai_callback(
    State(server): State<OpenAICallbackServerState>,
    Query(query): Query<OpenAICallbackQuery>,
) -> impl IntoResponse {
    let state_value = query.state.unwrap_or_default();
    if let Some(error) = query.error {
        let message = query.error_description.unwrap_or(error);
        let _ = server.app_handle.emit_to(
            EventTarget::webview("main"),
            "openai-oauth-complete",
            json!({"state": state_value, "error": message}),
        );
        return Html(callback_page(
            false,
            "OpenAI 授权被取消或拒绝。请返回 Aether 重试。",
        ));
    }
    let Some(code) = query.code else {
        return Html(callback_page(
            false,
            "授权回调缺少 code 参数。请返回 Aether 重新开始授权。",
        ));
    };
    let app_state = server.app_handle.state::<AppState>();
    let client = app_state.client.load_full();
    match app_state
        .oauth_sessions
        .complete_callback(&client, &code, &state_value)
        .await
    {
        Ok((session_id, account)) => match app_state.db.upsert_account(&account) {
            Ok((account, _)) => {
                let _ = server.app_handle.emit_to(
                    EventTarget::webview("main"),
                    "openai-oauth-complete",
                    json!({"state": state_value, "session_id": session_id, "account": account}),
                );
                Html(callback_page(
                    true,
                    "授权完成，账号已导入 Aether。现在可以关闭此页面。",
                ))
            }
            Err(error) => {
                let message = format!("保存 OpenAI 账号失败: {error}");
                let _ = server.app_handle.emit_to(
                    EventTarget::webview("main"),
                    "openai-oauth-complete",
                    json!({"state": state_value, "error": message}),
                );
                Html(callback_page(
                    false,
                    "保存账号失败，请返回 Aether 查看错误。",
                ))
            }
        },
        Err(error) => {
            let _ = server.app_handle.emit_to(
                EventTarget::webview("main"),
                "openai-oauth-complete",
                json!({"state": state_value, "error": error}),
            );
            Html(callback_page(
                false,
                "授权处理失败，请返回 Aether 查看错误。",
            ))
        }
    }
}

fn callback_page(success: bool, message: &str) -> String {
    let color = if success { "#16765d" } else { "#b42318" };
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Aether OpenAI 授权</title><main style=\"max-width:560px;margin:15vh auto;font-family:system-ui,sans-serif;color:{color}\"><h1>{message}</h1></main>"
    )
}

pub(super) async fn import_parsed_accounts(
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
    let client = state.client.load_full();

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
                match oauth::refresh_new_account(&client, &account).await {
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
pub(crate) async fn refresh_account(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Account, String> {
    let account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;
    let client = state.client.load_full();
    let refreshed = oauth::refresh_account(&client, &account).await;
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
pub(crate) async fn refresh_all_accounts(
    state: tauri::State<'_, AppState>,
) -> Result<ImportResult, String> {
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
    let client = state.client.load_full();
    for (index, account) in accounts.into_iter().enumerate() {
        match oauth::refresh_account(&client, &account).await {
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
pub(crate) async fn test_account(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    let mut account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "账号不存在".to_string())?;

    let url = if account.account_type == "oauth" {
        format!(
            "https://chatgpt.com/backend-api/codex/models?client_version={}",
            "0.144.1"
        )
    } else {
        let base = if account.base_url.trim().is_empty() {
            "https://api.openai.com"
        } else {
            account.base_url.trim_end_matches('/')
        };
        if base.ends_with("/v1") {
            format!("{base}/models")
        } else {
            format!("{base}/v1/models")
        }
    };
    let probe_path = reqwest::Url::parse(&url)
        .map(|url| url.path().to_string())
        .unwrap_or_else(|_| "/v1/models".to_string());
    let probe_log = logger::begin_probe(Arc::clone(&state.db), &account, &probe_path);
    let client = state.client.load_full();

    if account.account_type == "oauth"
        && (account.access_token.is_empty()
            || account
                .expires_at
                .map(|expires| expires <= chrono::Utc::now().timestamp() + 120)
                .unwrap_or(false))
        && !account.refresh_token.is_empty()
    {
        let refreshed = match oauth::refresh_account(&client, &account).await {
            Ok(refreshed) => refreshed,
            Err(message) => {
                if let Some(log) = &probe_log {
                    log.finish("error", Some(&message));
                }
                return Err(message);
            }
        };
        account = match state.db.update_oauth_tokens(&id, &refreshed) {
            Ok(account) => account,
            Err(error) => {
                let message = format!("保存刷新结果失败: {error}");
                if let Some(log) = &probe_log {
                    log.finish("error", Some(&message));
                }
                return Err(message);
            }
        };
    }

    let mut refreshed_after_unauthorized = false;
    loop {
        let token = if account.account_type == "oauth" {
            account.access_token.as_str()
        } else {
            account.api_key.as_str()
        };
        let mut request = client.get(&url).bearer_auth(token);
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

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                let message = format!("连接失败: {error}");
                if let Some(log) = &probe_log {
                    log.finish("error", Some(&message));
                }
                let _ = state.db.set_error(&id, &message);
                return Err(message);
            }
        };
        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED
            && account.account_type == "oauth"
            && !account.refresh_token.is_empty()
            && !refreshed_after_unauthorized
        {
            refreshed_after_unauthorized = true;
            let refreshed = match oauth::refresh_account(&client, &account).await {
                Ok(refreshed) => refreshed,
                Err(message) => {
                    if let Some(log) = &probe_log {
                        log.mark_response(status.as_u16());
                        log.finish("error", Some(&message));
                    }
                    let _ = state.db.set_error(&id, &message);
                    return Err(message);
                }
            };
            account = match state.db.update_oauth_tokens(&id, &refreshed) {
                Ok(account) => account,
                Err(error) => {
                    let message = format!("保存刷新结果失败: {error}");
                    if let Some(log) = &probe_log {
                        log.mark_response(status.as_u16());
                        log.finish("error", Some(&message));
                    }
                    let _ = state.db.set_error(&id, &message);
                    return Err(message);
                }
            };
            continue;
        }

        if let Some(log) = &probe_log {
            log.mark_response(status.as_u16());
        }
        if status.is_success() {
            if let Some(log) = &probe_log {
                log.finish("success", None);
            }
            let _ = state.db.set_error(&id, "");
            return Ok(format!("连接正常 ({status})"));
        }

        let body = response.text().await.unwrap_or_default();
        let message = format!(
            "连接失败 ({status}): {}",
            body.chars().take(240).collect::<String>()
        );
        if let Some(log) = &probe_log {
            log.finish("error", Some(&format!("连接失败 ({status})")));
        }
        let _ = state.db.set_error(&id, &message);
        return Err(message);
    }
}

#[tauri::command]
pub(crate) fn export_accounts(state: tauri::State<AppState>) -> Result<String, String> {
    let value = state.db.export_data().map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn reset_request_counts(state: tauri::State<AppState>) -> Result<u64, String> {
    state
        .db
        .reset_request_counts()
        .map_err(|error| error.to_string())
}
