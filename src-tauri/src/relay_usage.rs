use crate::db::Account;
use chrono::Utc;
use futures::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
const ERROR_PREVIEW_LIMIT: usize = 240;
const NEW_API_TOKEN_BODY_LIMIT: usize = 256 * 1024;
const NEW_API_LOG_BODY_LIMIT: usize = 2 * 1024 * 1024;
const DEFAULT_NEW_API_QUOTA_PER_UNIT: f64 = 500_000.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RelayUsageSummary {
    pub today_actual_cost: Option<f64>,
    pub last_30_days_actual_cost: Option<f64>,
    pub total_actual_cost: Option<f64>,
    pub quota_used: Option<f64>,
    pub quota_limit: Option<f64>,
    pub balance: Option<f64>,
    pub remaining: Option<f64>,
    pub plan: Option<String>,
    pub mode: String,
    pub fetched_at: i64,
    /// `generic` uses the relay's normal currency fields; `new_api` uses
    /// New API's integer quota units for the amount fields above.
    pub provider: String,
    pub unit: String,
    pub quota_per_unit: Option<f64>,
    pub unlimited_quota: bool,
    pub expires_at: Option<i64>,
    pub token_name: Option<String>,
    pub remote_request_count: Option<u64>,
    pub remote_last_request_at: Option<i64>,
    pub remote_last_model: Option<String>,
}

pub async fn query_usage(client: &Client, account: &Account) -> Result<RelayUsageSummary, String> {
    if account.account_type != "api_key" {
        return Err("只有中转站支持用量查询".to_string());
    }
    let api_key = account.api_key.trim();
    if api_key.is_empty() {
        return Err("中转站缺少 API Key".to_string());
    }
    let base_url = relay_base_url(&account.base_url)?;

    match tokio::time::timeout(QUERY_TIMEOUT, query_usage_inner(client, base_url, api_key)).await {
        Ok(result) => result,
        Err(_) => Err("中转站用量查询超时（15 秒）".to_string()),
    }
}

async fn query_usage_inner(
    client: &Client,
    base_url: Url,
    api_key: &str,
) -> Result<RelayUsageSummary, String> {
    let fetched_at = Utc::now().timestamp();

    // New API's token endpoint is outside the OpenAI-compatible `/v1` path.
    // Probe it first, then fall back to the generic relay contract so existing
    // providers keep their current behavior.
    let new_api_url = endpoint_url(&base_url, "/api/usage/token/");
    if let Ok((status, body)) = get_json(client, new_api_url, api_key, NEW_API_TOKEN_BODY_LIMIT).await
    {
        if status.is_success() {
            if let Some(token_usage) = parse_new_api_token_usage(&body) {
                return build_new_api_summary(client, &base_url, api_key, token_usage, fetched_at)
                    .await;
            }
        }
    }

    let url = usage_url_from_base(&base_url)?;
    let (status, body) = get_json(client, url, api_key, NEW_API_TOKEN_BODY_LIMIT)
        .await
        .map_err(|error| format!("中转站用量查询连接失败: {}", redact_secret(&error, api_key)))?;

    if !status.is_success() {
        let preview = response_preview(&body, api_key);
        return Err(if preview == "<空响应>" {
            format!("中转站用量查询失败 ({status})")
        } else {
            format!("中转站用量查询失败 ({status}): {preview}")
        });
    }

    parse_usage_response(&body, fetched_at).map_err(|error| {
        format!(
            "中转站用量响应解析失败 ({status}): {error}; 响应: {}",
            response_preview(&body, api_key)
        )
    })
}

async fn build_new_api_summary(
    client: &Client,
    base_url: &Url,
    api_key: &str,
    token_usage: NewApiTokenUsage,
    fetched_at: i64,
) -> Result<RelayUsageSummary, String> {
    let logs_url = endpoint_url(base_url, "/api/log/token");
    let status_url = endpoint_url(base_url, "/api/status");
    let (logs_result, status_result) = tokio::join!(
        get_json(client, logs_url, api_key, NEW_API_LOG_BODY_LIMIT),
        get_json(client, status_url, api_key, NEW_API_TOKEN_BODY_LIMIT),
    );

    let quota_per_unit = status_result
        .ok()
        .and_then(|(_, body)| parse_quota_per_unit(&body))
        .or(Some(DEFAULT_NEW_API_QUOTA_PER_UNIT));
    let log_summary = logs_result
        .ok()
        .and_then(|(status, body)| status.is_success().then(|| parse_new_api_logs(&body)));

    let unlimited_quota = token_usage.unlimited_quota;
    let quota_limit = (!unlimited_quota).then_some(token_usage.total_granted);
    let log_summary = log_summary.unwrap_or_default();

    Ok(RelayUsageSummary {
        today_actual_cost: log_summary.today_quota,
        last_30_days_actual_cost: log_summary.last_30_days_quota,
        total_actual_cost: Some(token_usage.total_used),
        quota_used: Some(token_usage.total_used),
        quota_limit,
        balance: None,
        remaining: Some(token_usage.total_available),
        plan: token_usage.name.clone(),
        mode: "new_api".to_string(),
        fetched_at,
        provider: "new_api".to_string(),
        unit: "quota".to_string(),
        quota_per_unit,
        unlimited_quota,
        expires_at: (token_usage.expires_at > 0).then_some(token_usage.expires_at),
        token_name: token_usage.name,
        remote_request_count: Some(log_summary.request_count),
        remote_last_request_at: log_summary.last_request_at,
        remote_last_model: log_summary.last_model,
    })
}

async fn get_json(
    client: &Client,
    url: Url,
    api_key: &str,
    max_bytes: usize,
) -> Result<(StatusCode, Vec<u8>), String> {
    let response = client
        .get(url)
        .bearer_auth(api_key)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("响应超过 {} KiB", max_bytes / 1024));
    }

    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("响应超过 {} KiB", max_bytes / 1024));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

#[derive(Debug, Clone)]
struct NewApiTokenUsage {
    name: Option<String>,
    total_granted: f64,
    total_used: f64,
    total_available: f64,
    unlimited_quota: bool,
    expires_at: i64,
}

#[derive(Debug, Clone, Default)]
struct NewApiLogSummary {
    request_count: u64,
    today_quota: Option<f64>,
    last_30_days_quota: Option<f64>,
    last_request_at: Option<i64>,
    last_model: Option<String>,
}

fn parse_new_api_token_usage(body: &[u8]) -> Option<NewApiTokenUsage> {
    let root = serde_json::from_slice::<Value>(body).ok()?;
    let data = root.get("data")?.as_object()?;
    let object = data.get("object").and_then(Value::as_str).unwrap_or_default();
    let data_value = data_value(data);
    let total_granted = number_at(&data_value, &[&["total_granted"]])?;
    let total_used = number_at(&data_value, &[&["total_used"]]).unwrap_or(0.0);
    let total_available = number_at(&data_value, &[&["total_available"]])
        .unwrap_or((total_granted - total_used).max(0.0));
    if object != "token_usage"
        && !data.contains_key("total_granted")
        && !data.contains_key("total_available")
    {
        return None;
    }
    Some(NewApiTokenUsage {
        name: data
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        total_granted,
        total_used,
        total_available,
        unlimited_quota: data
            .get("unlimited_quota")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        expires_at: number_at(&data_value, &[&["expires_at"]])
            .unwrap_or(0.0)
            .max(0.0) as i64,
    })
}

// Keep a borrowed `Value` helper so the existing path readers can parse a map.
fn data_value(data: &serde_json::Map<String, Value>) -> Value {
    Value::Object(data.clone())
}

fn parse_quota_per_unit(body: &[u8]) -> Option<f64> {
    let root = serde_json::from_slice::<Value>(body).ok()?;
    let payload = root.get("data").filter(|value| value.is_object()).unwrap_or(&root);
    number_at(payload, &[&["quota_per_unit"], &["quotaPerUnit"]])
        .filter(|value| *value > 0.0)
}

fn parse_new_api_logs(body: &[u8]) -> NewApiLogSummary {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return NewApiLogSummary::default();
    };
    let Some(items) = root.get("data").and_then(Value::as_array) else {
        return NewApiLogSummary::default();
    };
    let now = Utc::now().timestamp();
    let today_start = Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|value| value.and_utc().timestamp())
        .unwrap_or(now - 86_400);
    let month_start = now - 30 * 86_400;
    let mut summary = NewApiLogSummary::default();
    let mut today_total = 0.0;
    let mut month_total = 0.0;
    for item in items {
        if number_at(item, &[&["type"]]).unwrap_or(2.0) != 2.0 {
            continue;
        }
        summary.request_count = summary.request_count.saturating_add(1);
        let quota = number_at(item, &[&["quota"]]).unwrap_or(0.0);
        let created_at = number_at(item, &[&["created_at"], &["createdAt"]])
            .map(|value| value as i64);
        if let Some(created_at) = created_at {
            if created_at >= month_start {
                month_total += quota;
            }
            if created_at >= today_start {
                today_total += quota;
            }
            if summary
                .last_request_at
                .is_none_or(|previous| created_at > previous)
            {
                summary.last_request_at = Some(created_at);
                summary.last_model = item
                    .get("model_name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
            }
        }
    }
    if summary.request_count > 0 {
        summary.today_quota = Some(today_total);
        summary.last_30_days_quota = Some(month_total);
    }
    summary
}

fn relay_base_url(raw: &str) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("中转站缺少 API 地址".to_string());
    }
    let mut url = Url::parse(raw).map_err(|error| format!("中转站 API 地址无效: {error}"))?;
    if url.path().is_empty() {
        url.set_path("/");
    }
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("中转站 API 地址必须是有效的 HTTP(S) 地址".to_string());
    }
    Ok(url)
}

/// Build a site-root endpoint URL (e.g. `/api/status`) from the relay base,
/// dropping any trailing `/v1` since New API's management endpoints live at
/// the site root rather than under the OpenAI-compatible path.
fn endpoint_url(base_url: &Url, path: &str) -> Url {
    let mut url = base_url.clone();
    let base_path = base_url.path().trim_end_matches('/');
    let root = base_path.strip_suffix("/v1").unwrap_or(base_path);
    url.set_path(&format!("{root}{path}"));
    url.set_query(None);
    url.set_fragment(None);
    url
}

fn usage_url_from_base(base_url: &Url) -> Result<Url, String> {
    usage_url(base_url.as_str())
}

fn usage_url(base_url: &str) -> Result<Url, String> {
    let raw = base_url.trim();
    if raw.is_empty() {
        return Err("中转站缺少 API 地址".to_string());
    }

    let mut url = Url::parse(raw).map_err(|error| format!("中转站 API 地址无效: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("中转站 API 地址必须是有效的 HTTP(S) 地址".to_string());
    }

    let base_path = url.path().trim_end_matches('/');
    let path = if base_path.ends_with("/v1/usage") {
        base_path.to_string()
    } else if base_path.ends_with("/v1") {
        format!("{base_path}/usage")
    } else {
        format!("{base_path}/v1/usage")
    };
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn parse_usage_response(body: &[u8], fetched_at: i64) -> Result<RelayUsageSummary, String> {
    let root =
        serde_json::from_slice::<Value>(body).map_err(|error| format!("JSON 无效: {error}"))?;
    let payload = root
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(&root);

    let raw_mode = string_at(payload, &[&["mode"]]).unwrap_or_default();
    let has_quota =
        payload.get("quota").is_some_and(Value::is_object) || raw_mode == "quota_limited";
    let has_subscription =
        payload.get("subscription").is_some_and(Value::is_object) || raw_mode == "subscription";
    let has_wallet = number_at(payload, &[&["balance"], &["wallet", "balance"]]).is_some()
        || raw_mode == "wallet";
    let has_usage = payload.get("usage").is_some() || payload.get("daily_usage").is_some();
    if !has_quota && !has_subscription && !has_wallet && !has_usage {
        return Err("缺少可识别的用量字段".to_string());
    }

    let mode = if has_quota {
        "quota_limited"
    } else if has_subscription {
        "subscription"
    } else if has_wallet {
        "wallet"
    } else {
        raw_mode.as_str()
    };
    let mode = if mode.is_empty() { "unknown" } else { mode }.to_string();

    let today_actual_cost = number_at(
        payload,
        &[
            &["usage", "today", "actual_cost"],
            &["usage", "today", "actualCost"],
            &["usage", "today", "cost"],
            &["today", "actual_cost"],
        ],
    );
    let total_actual_cost = number_at(
        payload,
        &[
            &["usage", "total", "actual_cost"],
            &["usage", "total", "actualCost"],
            &["usage", "total", "cost"],
            &["total", "actual_cost"],
        ],
    );
    let last_30_days_actual_cost =
        daily_actual_cost(payload).or_else(|| array_actual_cost(payload, "model_stats"));

    let mut quota_used = number_at(
        payload,
        &[&["quota", "used"], &["quota_used"], &["quotaUsed"]],
    );
    let mut quota_limit = number_at(
        payload,
        &[&["quota", "limit"], &["quota_limit"], &["quotaLimit"]],
    );
    if has_subscription && quota_limit.is_none() {
        for (used_key, limit_key) in [
            ("monthly_usage_usd", "monthly_limit_usd"),
            ("weekly_usage_usd", "weekly_limit_usd"),
            ("daily_usage_usd", "daily_limit_usd"),
        ] {
            let limit = number_at(payload, &[&["subscription", limit_key]]);
            if limit.is_some_and(|value| value > 0.0) {
                quota_used = number_at(payload, &[&["subscription", used_key]]);
                quota_limit = limit;
                break;
            }
        }
    }

    let balance = number_at(payload, &[&["balance"], &["wallet", "balance"]]);
    let remaining = number_at(
        payload,
        &[
            &["quota", "remaining"],
            &["remaining"],
            &["subscription", "remaining_usd"],
            &["wallet", "remaining"],
        ],
    )
    .or_else(|| match (quota_limit, quota_used) {
        (Some(limit), Some(used)) => Some((limit - used).max(0.0)),
        _ => balance,
    });

    Ok(RelayUsageSummary {
        today_actual_cost,
        last_30_days_actual_cost,
        total_actual_cost,
        quota_used,
        quota_limit,
        balance,
        remaining,
        plan: string_at(
            payload,
            &[
                &["planName"],
                &["plan_name"],
                &["plan"],
                &["subscription", "plan_name"],
                &["subscription", "name"],
            ],
        ),
        mode,
        fetched_at,
        provider: "generic".to_string(),
        unit: "usd".to_string(),
        quota_per_unit: None,
        unlimited_quota: false,
        expires_at: None,
        token_name: None,
        remote_request_count: None,
        remote_last_request_at: None,
        remote_last_model: None,
    })
}

fn daily_actual_cost(payload: &Value) -> Option<f64> {
    array_actual_cost(payload, "daily_usage")
}

fn array_actual_cost(payload: &Value, field: &str) -> Option<f64> {
    let items = payload.get(field)?.as_array()?;
    let values = items
        .iter()
        .filter_map(|item| number_at(item, &[&["actual_cost"], &["actualCost"], &["cost"]]))
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.into_iter().sum())
}

fn number_at(value: &Value, paths: &[&[&str]]) -> Option<f64> {
    paths.iter().find_map(|path| {
        let value = value_at(value, path)?;
        let number = match value {
            Value::Number(number) => number.as_f64(),
            Value::String(raw) => raw.trim().parse::<f64>().ok(),
            _ => None,
        }?;
        number.is_finite().then_some(number)
    })
}

fn string_at(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| {
        value_at(value, path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    Some(cursor)
}

fn redact_secret(value: &str, secret: &str) -> String {
    if secret.is_empty() {
        value.to_string()
    } else {
        value.replace(secret, "[REDACTED]")
    }
}

fn response_preview(body: &[u8], api_key: &str) -> String {
    let text = redact_secret(&String::from_utf8_lossy(body), api_key);
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "<空响应>".to_string()
    } else {
        compact.chars().take(ERROR_PREVIEW_LIMIT).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_usage_url_from_root_and_v1_base() {
        assert_eq!(
            usage_url("https://relay.example.com").unwrap().as_str(),
            "https://relay.example.com/v1/usage"
        );
        assert_eq!(
            usage_url("https://relay.example.com/openai/v1/")
                .unwrap()
                .as_str(),
            "https://relay.example.com/openai/v1/usage"
        );
        assert_eq!(
            usage_url("https://relay.example.com/v1/usage?old=1")
                .unwrap()
                .as_str(),
            "https://relay.example.com/v1/usage"
        );
    }

    #[test]
    fn parses_quota_limited_usage_with_number_strings() {
        let summary = parse_usage_response(
            br#"{
                "mode":"quota_limited",
                "quota":{"used":"15.32","limit":25,"remaining":"9.68"},
                "usage":{
                    "today":{"actual_cost":"0.0000"},
                    "total":{"actual_cost":"15.3169"}
                },
                "daily_usage":[
                    {"actual_cost":"10.1"},
                    {"actual_cost":5.2169}
                ]
            }"#,
            1_700_000_000,
        )
        .unwrap();

        assert_eq!(summary.mode, "quota_limited");
        assert_eq!(summary.today_actual_cost, Some(0.0));
        assert!((summary.last_30_days_actual_cost.unwrap() - 15.3169).abs() < f64::EPSILON * 8.0);
        assert_eq!(summary.total_actual_cost, Some(15.3169));
        assert_eq!(summary.quota_used, Some(15.32));
        assert_eq!(summary.quota_limit, Some(25.0));
        assert_eq!(summary.remaining, Some(9.68));
        assert_eq!(summary.fetched_at, 1_700_000_000);
    }

    #[test]
    fn parses_subscription_usage_and_uses_longest_limit() {
        let summary = parse_usage_response(
            r#"{
                "data": {
                    "mode":"unrestricted",
                    "planName":"团队月度套餐",
                    "remaining":"18.50",
                    "subscription":{
                        "daily_usage_usd":"1.5",
                        "daily_limit_usd":"5",
                        "weekly_usage_usd":4,
                        "weekly_limit_usd":20,
                        "monthly_usage_usd":"11.5",
                        "monthly_limit_usd":"30"
                    },
                    "usage":{
                        "today":{"actual_cost":1.25},
                        "total":{"actual_cost":"12.75"}
                    },
                    "model_stats":[
                        {"actual_cost":"5.25"},
                        {"actual_cost":7.5}
                    ]
                }
            }"#
            .as_bytes(),
            42,
        )
        .unwrap();

        assert_eq!(summary.mode, "subscription");
        assert_eq!(summary.plan.as_deref(), Some("团队月度套餐"));
        assert_eq!(summary.quota_used, Some(11.5));
        assert_eq!(summary.quota_limit, Some(30.0));
        assert_eq!(summary.remaining, Some(18.5));
        assert_eq!(summary.last_30_days_actual_cost, Some(12.75));
    }

    #[test]
    fn parses_wallet_usage_and_balance() {
        let summary = parse_usage_response(
            r#"{
                "mode":"unrestricted",
                "planName":"钱包余额",
                "balance":"7.25",
                "remaining":7.25,
                "usage":{
                    "today":{"actualCost":"0.50"},
                    "total":{"actualCost":9.75}
                }
            }"#
            .as_bytes(),
            99,
        )
        .unwrap();

        assert_eq!(summary.mode, "wallet");
        assert_eq!(summary.plan.as_deref(), Some("钱包余额"));
        assert_eq!(summary.balance, Some(7.25));
        assert_eq!(summary.remaining, Some(7.25));
        assert_eq!(summary.today_actual_cost, Some(0.5));
        assert_eq!(summary.total_actual_cost, Some(9.75));
    }
}
