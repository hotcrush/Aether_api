use crate::db::{Account, Db};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

const RATE_SYNC_INTERVAL: Duration = Duration::from_secs(30 * 60);
const RATE_SYNC_MAX_MULTIPLIER: f64 = 100.0;
const RATE_SYNC_MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
struct UpstreamBillingSnapshot {
    object: String,
    schema_version: u32,
    billing_scope: String,
    resolved_rate_multiplier: f64,
}

pub async fn sync_account_rate_multiplier(
    db: &Db,
    client: &reqwest::Client,
    account_id: &str,
) -> Result<f64, String> {
    let account = db
        .get_account(account_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "上游不存在".to_string())?;
    let multiplier = fetch_rate_multiplier(client, &account).await?;
    if !db
        .set_rate_multiplier_from_sync(account_id, multiplier)
        .map_err(|error| error.to_string())?
    {
        return Err("请先开启该中转站的自动倍率同步".to_string());
    }
    Ok(multiplier)
}

pub fn start_rate_sync(db: Arc<Db>, client: Arc<arc_swap::ArcSwap<reqwest::Client>>) {
    tauri::async_runtime::spawn(async move {
        loop {
            let accounts = match db.list_rate_sync_accounts() {
                Ok(accounts) => accounts,
                Err(error) => {
                    warn!(%error, "读取待同步倍率的中转站失败");
                    Vec::new()
                }
            };
            for account in accounts.into_iter().take(20) {
                let account_id = account.id.clone();
                let active_client = client.load_full();
                match fetch_rate_multiplier(&active_client, &account).await {
                    Ok(multiplier) => {
                        if let Err(error) =
                            db.set_rate_multiplier_from_sync(&account_id, multiplier)
                        {
                            warn!(account_id = %account_id, %error, "写入上游倍率失败");
                        }
                    }
                    Err(error) => {
                        warn!(account_id = %account_id, %error, "上游倍率同步失败");
                    }
                }
            }
            tokio::time::sleep(RATE_SYNC_INTERVAL).await;
        }
    });
}

async fn fetch_rate_multiplier(client: &reqwest::Client, account: &Account) -> Result<f64, String> {
    if account.account_type != "api_key" || account.api_key.trim().is_empty() {
        return Err("只有 API Key 中转站支持上游倍率同步".to_string());
    }
    let base_url = account.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("中转站缺少 Base URL".to_string());
    }
    let url = if base_url.ends_with("/v1") {
        format!("{base_url}/sub2api/billing")
    } else {
        format!("{base_url}/v1/sub2api/billing")
    };
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        client
            .get(url)
            .header("accept", "application/json")
            .bearer_auth(&account.api_key)
            .send(),
    )
    .await
    .map_err(|_| "上游倍率探测超时".to_string())?
    .map_err(|error| format!("上游倍率探测失败: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("上游倍率探测返回 HTTP {}", response.status()));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("读取上游倍率响应失败: {error}"))?;
    if body.len() > RATE_SYNC_MAX_BODY_BYTES {
        return Err("上游倍率响应过大".to_string());
    }
    let snapshot: UpstreamBillingSnapshot =
        serde_json::from_slice(&body).map_err(|_| "上游倍率响应格式无效".to_string())?;
    if snapshot.object != "sub2api.key_billing"
        || snapshot.schema_version != 1
        || snapshot.billing_scope != "token"
    {
        return Err("上游不支持 Sub2API 倍率探测".to_string());
    }
    let multiplier = snapshot.resolved_rate_multiplier;
    if !multiplier.is_finite() || !(0.0..=RATE_SYNC_MAX_MULTIPLIER).contains(&multiplier) {
        return Err("上游声明的倍率必须在 0 到 100 之间".to_string());
    }
    if multiplier == 0.0 {
        return Err("上游声明的免费倍率不会自动写入".to_string());
    }
    Ok((multiplier * 10_000.0).round() / 10_000.0)
}
