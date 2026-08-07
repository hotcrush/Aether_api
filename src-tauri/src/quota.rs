use crate::db::Account;
use crate::quota_headers::{parse_codex_quota_headers, CodexQuotaWindowSnapshot};
use chrono::Utc;
use reqwest::{header::HeaderMap, Client};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use std::fmt;
use std::time::Duration;

const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
pub(crate) const QUOTA_CACHE_KEY: &str = "aether:quota_cache";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitWindow {
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub limit_window_seconds: Option<i64>,
    pub reset_after_seconds: Option<i64>,
    pub reset_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_requests: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_requests_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_tokens_limit: Option<i64>,
}

impl RateLimitWindow {
    fn normalize(&mut self, fetched_at: i64) {
        self.remaining_percent = self
            .used_percent
            .map(|used| (100.0 - used).clamp(0.0, 100.0));
        let reset_at_is_missing = self.reset_at.filter(|value| *value > 0).is_none();
        if reset_at_is_missing {
            self.reset_at = self
                .reset_after_seconds
                .filter(|seconds| *seconds > 0)
                .map(|seconds| fetched_at.saturating_add(seconds));
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimit {
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub primary_window: Option<RateLimitWindow>,
    pub secondary_window: Option<RateLimitWindow>,
}

impl RateLimit {
    fn normalize(&mut self, fetched_at: i64) {
        if let Some(window) = self.primary_window.as_mut() {
            window.normalize(fetched_at);
        }
        if let Some(window) = self.secondary_window.as_mut() {
            window.normalize(fetched_at);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdditionalRateLimit {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub limit_name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub metered_feature: String,
    pub rate_limit: Option<RateLimit>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitResetCredit {
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitResetCredits {
    pub available_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<Vec<RateLimitResetCredit>>,
}

impl RateLimitResetCredits {
    fn normalize(&mut self, fetched_at: i64) {
        let Some(credits) = self.credits.as_mut() else {
            return;
        };
        credits.retain(|credit| {
            credit.expires_at.as_deref().is_some_and(|expires_at| {
                chrono::DateTime::parse_from_rfc3339(expires_at)
                    .map(|value| value.timestamp() > fetched_at)
                    .unwrap_or(true)
            })
        });
        let remaining = i64::try_from(credits.len()).unwrap_or(i64::MAX);
        self.available_count = Some(
            self.available_count
                .unwrap_or(remaining)
                .clamp(0, remaining),
        );
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuotaUsage {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub user_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub account_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub email: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub plan_type: String,
    pub rate_limit: Option<RateLimit>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub additional_rate_limits: Vec<AdditionalRateLimit>,
    pub rate_limit_reset_credits: Option<RateLimitResetCredits>,
    #[serde(default)]
    pub fetched_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_limit_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_limit_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_sample_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_sample_requests: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_sample_used_percent: Option<f64>,
}

impl QuotaUsage {
    fn normalize(&mut self, account: &Account) {
        self.fetched_at = Utc::now().timestamp();
        if self.account_id.is_empty() {
            self.account_id.clone_from(&account.chatgpt_account_id);
        }
        if self.email.is_empty() {
            self.email.clone_from(&account.email);
        }
        if self.plan_type.is_empty() {
            self.plan_type.clone_from(&account.plan_type);
        }
        if let Some(rate_limit) = self.rate_limit.as_mut() {
            rate_limit.normalize(self.fetched_at);
        }
        for additional in &mut self.additional_rate_limits {
            if let Some(rate_limit) = additional.rate_limit.as_mut() {
                rate_limit.normalize(self.fetched_at);
            }
        }
        if let Some(reset_credits) = self.rate_limit_reset_credits.as_mut() {
            reset_credits.normalize(self.fetched_at);
        }
    }
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug)]
pub struct QueryError {
    pub status: Option<u16>,
    pub message: String,
}

impl QueryError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaQueryResult {
    pub account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn query_usage(
    client: &Client,
    account: &Account,
    codex_version: &str,
) -> Result<QuotaUsage, QueryError> {
    if account.account_type != "oauth" {
        return Err(QueryError::new("只有 OpenAI OAuth 账号支持额度查询"));
    }
    if account.access_token.trim().is_empty() {
        return Err(QueryError::new("账号缺少 access_token"));
    }
    if account.chatgpt_account_id.trim().is_empty() {
        return Err(QueryError::new(
            "账号缺少 ChatGPT Account ID，请重新导入或授权",
        ));
    }

    let request = client
        .get(CHATGPT_USAGE_URL)
        .timeout(Duration::from_secs(20))
        .bearer_auth(&account.access_token)
        .header("chatgpt-account-id", &account.chatgpt_account_id)
        .header("openai-beta", "codex-1")
        .header("oai-language", "zh-CN")
        .header("accept", "application/json")
        .header("sec-fetch-site", "none")
        .header("sec-fetch-mode", "no-cors")
        .header("sec-fetch-dest", "empty")
        .header("priority", "u=4, i");
    let response = crate::codex_identity::apply_identity(request, codex_version)
        .send()
        .await
        .map_err(|error| QueryError::new(format!("额度查询连接失败: {error}")))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let redacted = body.replace(&account.access_token, "[REDACTED]");
        let detail = redacted.chars().take(240).collect::<String>();
        let message = if detail.trim().is_empty() {
            format!("额度查询失败 ({status})")
        } else {
            format!("额度查询失败 ({status}): {detail}")
        };
        return Err(QueryError {
            status: Some(status.as_u16()),
            message,
        });
    }

    let body = response.bytes().await.map_err(|error| QueryError {
        status: Some(status.as_u16()),
        message: format!("额度响应读取失败 ({status}): {error}"),
    })?;
    let mut usage = parse_usage_response(&body).map_err(|error| QueryError {
        status: Some(status.as_u16()),
        message: format!(
            "额度响应解析失败 ({status}): {error}; 响应: {}",
            response_preview(&body, &account.access_token)
        ),
    })?;
    usage.normalize(account);
    Ok(usage)
}

pub(crate) fn full_cache_entry_json(usage: &QuotaUsage) -> Result<String, serde_json::Error> {
    let fetched_at = if usage.fetched_at > 0 {
        usage.fetched_at
    } else {
        Utc::now().timestamp()
    };
    serde_json::to_string(&json!({
        "quota": usage,
        "cached_at": fetched_at.saturating_mul(1_000),
    }))
}

/// Build a partial cache entry from Codex response headers. The DB layer deep
/// merges this patch into the last complete `/wham/usage` snapshot, so fields
/// such as additional limits and reset credits are never discarded.
pub(crate) fn codex_header_cache_entry_json(
    account: &Account,
    headers: &HeaderMap,
) -> Option<String> {
    if account.account_type != "oauth" {
        return None;
    }
    let snapshot = parse_codex_quota_headers(headers)?;
    let mut rate_limit = Map::new();
    if let Some(window) = snapshot.primary_window.as_ref() {
        rate_limit.insert("primary_window".to_string(), window_value(window));
    }
    if let Some(window) = snapshot.secondary_window.as_ref() {
        rate_limit.insert("secondary_window".to_string(), window_value(window));
    }
    if rate_limit.is_empty() {
        return None;
    }

    let mut quota = Map::new();
    if !account.chatgpt_user_id.is_empty() {
        quota.insert("user_id".to_string(), json!(account.chatgpt_user_id));
    }
    if !account.chatgpt_account_id.is_empty() {
        quota.insert("account_id".to_string(), json!(account.chatgpt_account_id));
    }
    if !account.email.is_empty() {
        quota.insert("email".to_string(), json!(account.email));
    }
    if !account.plan_type.is_empty() {
        quota.insert("plan_type".to_string(), json!(account.plan_type));
    }
    quota.insert("rate_limit".to_string(), Value::Object(rate_limit));
    quota.insert("fetched_at".to_string(), json!(snapshot.fetched_at));
    // Keep this private diagnostic available to future schedulers without
    // changing the public AccountQuota shape consumed by the UI.
    if let Some(value) = snapshot.primary_over_secondary_limit_percent {
        quota.insert(
            "codex_primary_over_secondary_limit_percent".to_string(),
            json!(value),
        );
    }

    serde_json::to_string(&json!({
        "quota": Value::Object(quota),
        "cached_at": snapshot.fetched_at.saturating_mul(1_000),
    }))
    .ok()
}

fn window_value(window: &CodexQuotaWindowSnapshot) -> Value {
    serde_json::to_value(window).unwrap_or_else(|_| Value::Object(Map::new()))
}

fn parse_usage_response(body: &[u8]) -> Result<QuotaUsage, String> {
    let root =
        serde_json::from_slice::<Value>(body).map_err(|error| format!("JSON 无效: {error}"))?;
    let payload = root
        .get("data")
        .or_else(|| root.get("usage"))
        .unwrap_or(&root);
    serde_json::from_value(payload.clone()).map_err(|error| format!("字段不兼容: {error}"))
}

fn response_preview(body: &[u8], access_token: &str) -> String {
    let text = String::from_utf8_lossy(body).replace(access_token, "[REDACTED]");
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "<空响应>".to_string()
    } else {
        compact.chars().take(320).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth_account() -> Account {
        Account {
            id: "local-account".to_string(),
            name: "OAuth".to_string(),
            account_type: "oauth".to_string(),
            api_key: String::new(),
            access_token: String::new(),
            refresh_token: String::new(),
            refreshable: false,
            id_token: String::new(),
            client_id: String::new(),
            credential_masked: String::new(),
            base_url: String::new(),
            chatgpt_account_id: "chatgpt-account".to_string(),
            chatgpt_user_id: "chatgpt-user".to_string(),
            email: "user@example.com".to_string(),
            plan_type: "plus".to_string(),
            expires_at: None,
            priority: 0,
            models: Vec::new(),
            weight: 1,
            concurrency: 10,
            rate_multiplier: 1.0,
            auto_sync_rate_multiplier: false,
            status: "active".to_string(),
            last_error: String::new(),
            last_used_at: None,
            request_count: 0,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn calculates_remaining_percent_and_reset_time() {
        let mut window = RateLimitWindow {
            used_percent: Some(37.5),
            remaining_percent: None,
            limit_window_seconds: Some(18_000),
            reset_after_seconds: Some(90),
            reset_at: None,
            num_requests: None,
            num_requests_limit: None,
            num_tokens: None,
            num_tokens_limit: None,
        };
        window.normalize(1_000);
        assert_eq!(window.remaining_percent, Some(62.5));
        assert_eq!(window.reset_at, Some(1_090));
    }

    #[test]
    fn removes_expired_reset_credits_from_cached_usage() {
        let mut credits = RateLimitResetCredits {
            available_count: Some(3),
            credits: Some(vec![
                RateLimitResetCredit {
                    expires_at: Some("2026-08-07T00:00:00Z".to_string()),
                },
                RateLimitResetCredit {
                    expires_at: Some("2026-08-09T00:00:00Z".to_string()),
                },
            ]),
        };
        credits.normalize(1_786_147_200);
        assert_eq!(credits.available_count, Some(1));
        assert_eq!(credits.credits.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn parses_nullable_additional_limits() {
        let usage = parse_usage_response(
            br#"{
                "plan_type":"plus",
                "rate_limit":{
                    "allowed":true,
                    "limit_reached":false,
                    "primary_window":{
                        "used_percent":12,
                        "limit_window_seconds":18000,
                        "reset_after_seconds":600,
                        "reset_at":1700000000
                    }
                },
                "additional_rate_limits":null
            }"#,
        )
        .unwrap();

        assert_eq!(usage.plan_type, "plus");
        assert!(usage.additional_rate_limits.is_empty());
        assert_eq!(
            usage
                .rate_limit
                .and_then(|limit| limit.primary_window)
                .and_then(|window| window.used_percent),
            Some(12.0)
        );
    }

    #[test]
    fn parses_wrapped_usage_with_null_metadata() {
        let usage = parse_usage_response(
            br#"{
                "data":{
                    "user_id":null,
                    "email":null,
                    "plan_type":"team",
                    "rate_limit":null,
                    "additional_rate_limits":[]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(usage.user_id, "");
        assert_eq!(usage.email, "");
        assert_eq!(usage.plan_type, "team");
    }

    #[test]
    fn builds_partial_cache_entry_from_codex_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-primary-used-percent", "80".parse().unwrap());
        headers.insert("x-codex-primary-window-minutes", "10080".parse().unwrap());
        headers.insert(
            "x-codex-primary-reset-after-seconds",
            "3600".parse().unwrap(),
        );
        let entry = codex_header_cache_entry_json(&oauth_account(), &headers).unwrap();
        let entry: Value = serde_json::from_str(&entry).unwrap();
        assert_eq!(
            entry.pointer("/quota/account_id"),
            Some(&json!("chatgpt-account"))
        );
        assert_eq!(
            entry.pointer("/quota/rate_limit/primary_window/remaining_percent"),
            Some(&json!(20.0))
        );
        assert_eq!(
            entry.pointer("/quota/rate_limit/primary_window/limit_window_seconds"),
            Some(&json!(604_800))
        );
        assert!(entry.pointer("/quota/additional_rate_limits").is_none());
    }
}
