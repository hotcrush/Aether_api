use super::AppState;
use crate::db::{Account, Db};
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
        let client = state.client.load_full();
        let refreshed = oauth::refresh_account(&client, &account).await?;
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
    let client = state.client.load_full();
    let mut usage = match quota::query_usage(&client, &account).await {
        Ok(usage) => usage,
        Err(error) if error.status == Some(401) && !account.refresh_token.is_empty() => {
            let refreshed = oauth::refresh_account(&client, &account).await?;
            let refreshed = state
                .db
                .update_oauth_tokens(id, &refreshed)
                .map_err(|db_error| format!("保存刷新结果失败: {db_error}"))?;
            quota::query_usage(&client, &refreshed)
                .await
                .map_err(|retry_error| retry_error.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    if let Err(error) = attach_estimated_limit(&state.db, &account, &mut usage) {
        warn!(account_id = id, %error, "测算账号额度失败");
    }
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

fn attach_estimated_limit(
    db: &Db,
    account: &Account,
    usage: &mut quota::QuotaUsage,
) -> Result<(), String> {
    let mut windows = quota_estimate_windows(usage);
    windows.sort_by(|left, right| right.1.cmp(&left.1));

    for (window, duration) in windows {
        let used_percent = window
            .used_percent
            .or_else(|| window.remaining_percent.map(|remaining| 100.0 - remaining))
            .map(|value| value.clamp(0.0, 100.0));
        let Some(used_percent) = used_percent.filter(|value| *value >= 1.0) else {
            continue;
        };
        let reset_at = window.reset_at.filter(|value| *value > 0).or_else(|| {
            window
                .reset_after_seconds
                .filter(|value| *value >= 0)
                .map(|value| usage.fetched_at.saturating_add(value))
        });
        let Some(window_started_at) = reset_at.map(|value| value.saturating_sub(duration)) else {
            continue;
        };
        let (request_count, sample_cost) = db
            .account_estimated_cost_since(&account.id, window_started_at)
            .map_err(|error| error.to_string())?;
        if request_count <= 0 || !sample_cost.is_finite() || sample_cost <= 0.0 {
            continue;
        }
        let estimated_limit = sample_cost * 100.0 / used_percent;
        if !estimated_limit.is_finite() || estimated_limit <= 0.0 {
            continue;
        }
        usage.estimated_limit_usd = Some(estimated_limit);
        usage.estimated_limit_window =
            Some(if duration <= 21_600 { "5h" } else { "7d" }.to_string());
        usage.estimated_sample_cost_usd = Some(sample_cost);
        usage.estimated_sample_requests = Some(request_count);
        usage.estimated_sample_used_percent = Some(used_percent);
        break;
    }
    Ok(())
}

fn quota_estimate_windows(usage: &quota::QuotaUsage) -> Vec<(quota::RateLimitWindow, i64)> {
    fn from_limit(limit: Option<&quota::RateLimit>) -> Vec<(quota::RateLimitWindow, i64)> {
        let Some(limit) = limit else {
            return Vec::new();
        };
        [
            (limit.primary_window.as_ref(), 604_800),
            (limit.secondary_window.as_ref(), 18_000),
        ]
        .into_iter()
        .filter_map(|(window, fallback)| {
            let window = window?;
            let duration = window
                .limit_window_seconds
                .filter(|value| *value > 0)
                .unwrap_or(fallback);
            Some((window.clone(), duration))
        })
        .collect()
    }

    let windows = from_limit(usage.rate_limit.as_ref());
    if !windows.is_empty() {
        return windows;
    }
    for additional in &usage.additional_rate_limits {
        let windows = from_limit(additional.rate_limit.as_ref());
        if !windows.is_empty() {
            return windows;
        }
    }
    Vec::new()
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
    let client = state.client.load_full();
    relay_usage::query_usage(&client, &account).await
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
