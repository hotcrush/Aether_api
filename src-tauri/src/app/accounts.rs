use super::AppState;
use crate::account_import::{ImportMessage, ImportResult};
use crate::db::{Account, NewAccount, UpsertAction};
use crate::{logger, oauth};
use std::sync::Arc;

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
pub(crate) async fn refresh_account(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Account, String> {
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

    if account.account_type == "oauth"
        && (account.access_token.is_empty()
            || account
                .expires_at
                .map(|expires| expires <= chrono::Utc::now().timestamp() + 120)
                .unwrap_or(false))
        && !account.refresh_token.is_empty()
    {
        let refreshed = match oauth::refresh_account(&state.client, &account).await {
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
        let mut request = state.client.get(&url).bearer_auth(token);
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
            let refreshed = match oauth::refresh_account(&state.client, &account).await {
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
