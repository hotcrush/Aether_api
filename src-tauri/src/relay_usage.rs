use crate::db::Account;
use chrono::Utc;
use reqwest::{Client, Url};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
const ERROR_PREVIEW_LIMIT: usize = 240;

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
}

pub async fn query_usage(client: &Client, account: &Account) -> Result<RelayUsageSummary, String> {
    if account.account_type != "api_key" {
        return Err("只有中转站支持用量查询".to_string());
    }
    let api_key = account.api_key.trim();
    if api_key.is_empty() {
        return Err("中转站缺少 API Key".to_string());
    }
    let url = usage_url(&account.base_url)?;

    match tokio::time::timeout(QUERY_TIMEOUT, query_usage_inner(client, url, api_key)).await {
        Ok(result) => result,
        Err(_) => Err("中转站用量查询超时（15 秒）".to_string()),
    }
}

async fn query_usage_inner(
    client: &Client,
    url: Url,
    api_key: &str,
) -> Result<RelayUsageSummary, String> {
    let response = client
        .get(url)
        .bearer_auth(api_key)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| {
            format!(
                "中转站用量查询连接失败: {}",
                redact_secret(&error.to_string(), api_key)
            )
        })?;

    let status = response.status();
    let body = response.bytes().await.map_err(|error| {
        format!(
            "中转站用量响应读取失败 ({status}): {}",
            redact_secret(&error.to_string(), api_key)
        )
    })?;

    if !status.is_success() {
        let preview = response_preview(&body, api_key);
        return Err(if preview == "<空响应>" {
            format!("中转站用量查询失败 ({status})")
        } else {
            format!("中转站用量查询失败 ({status}): {preview}")
        });
    }

    parse_usage_response(&body, Utc::now().timestamp()).map_err(|error| {
        format!(
            "中转站用量响应解析失败 ({status}): {error}; 响应: {}",
            response_preview(&body, api_key)
        )
    })
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
