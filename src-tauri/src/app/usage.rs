use super::AppState;
use crate::db::Account;
use crate::{oauth, quota, relay_usage};
use futures::StreamExt;
use tracing::warn;

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
pub(super) async fn query_account_quota(
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
    let usage = match quota::query_usage(&state.client, &account).await {
        Ok(usage) => usage,
        Err(error) if error.status == Some(401) && !account.refresh_token.is_empty() => {
            let refreshed = oauth::refresh_account(&state.client, &account).await?;
            let refreshed = state
                .db
                .update_oauth_tokens(id, &refreshed)
                .map_err(|db_error| format!("保存刷新结果失败: {db_error}"))?;
            quota::query_usage(&state.client, &refreshed)
                .await
                .map_err(|retry_error| retry_error.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    match quota::full_cache_entry_json(&usage) {
        Ok(entry) => {
            if let Err(error) = state
                .db
                .set_json_setting_entries_async(
                    quota::QUOTA_CACHE_KEY,
                    vec![(id.to_string(), entry)],
                )
                .await
            {
                warn!(account_id = id, %error, "保存额度缓存失败");
            }
        }
        Err(error) => warn!(account_id = id, %error, "序列化额度缓存失败"),
    }
    Ok(usage)
}

#[tauri::command]
pub(super) async fn query_all_quotas(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<quota::QuotaQueryResult>, String> {
    const QUERY_CONCURRENCY: usize = 3;

    let account_ids = state
        .db
        .list_accounts()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|account| account.account_type == "oauth" && account.status == "active")
        .map(|account| account.id)
        .collect::<Vec<_>>();

    Ok(futures::stream::iter(account_ids)
        .map(|account_id| {
            let state = &state;
            async move {
                match query_quota_for_id(state, &account_id).await {
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
                }
            }
        })
        .buffered(QUERY_CONCURRENCY)
        .collect()
        .await)
}

#[tauri::command]
pub(super) async fn query_relay_usage(
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
pub(super) fn open_relay_site(state: tauri::State<AppState>, id: String) -> Result<String, String> {
    let account = state
        .db
        .get_account(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "中转站不存在".to_string())?;
    if account.account_type != "api_key" {
        return Err("只有中转站账号支持打开网页".to_string());
    }

    let site_url = relay_site_url(&account.base_url)?;
    Ok(site_url.to_string())
}

#[cfg(test)]
mod tests {
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
