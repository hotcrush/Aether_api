use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProduct {
    pub id: String,
    pub goods_key: String,
    pub shop_token: String,
    pub shop_name: String,
    pub shop_url: String,
    pub name: String,
    pub url: String,
    pub price: f64,
    pub fee: f64,
    pub fee_rate: f64,
    pub fee_payer: i64,
    pub total_price: f64,
    pub market_price: f64,
    pub stock_count: i64,
    pub source_category: String,
    pub category: Option<String>,
    pub match_terms: Vec<String>,
    pub verification_status: String,
    pub missing_count: i64,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShop {
    pub platform: String,
    pub token: String,
    pub fallback_name: String,
    pub name: String,
    pub enabled: bool,
    pub ok: bool,
    pub error: Option<String>,
    pub failure_count: i64,
    pub blocked_until: Option<String>,
    pub fee_rate: f64,
    pub fee_payer: i64,
    pub fee_checked_at: Option<String>,
    pub profile_checked_at: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_success_at: Option<String>,
    pub goods_types: Vec<String>,
    pub product_count: i64,
    pub total_stock: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketProtection {
    pub mode: String,
    pub consecutive_failures: i64,
    pub circuit_open_until: Option<String>,
    pub circuit_reason: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_request_at: Option<String>,
    pub last_success_at: Option<String>,
    pub request_timestamps: Vec<String>,
    pub active_api_base: String,
    pub fallback_used: bool,
    pub data_mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketSnapshot {
    pub status: String,
    pub products: Vec<MarketProduct>,
    pub shops: Vec<MarketShop>,
    pub protection: MarketProtection,
    pub last_checked_at: Option<String>,
    pub next_refresh_at: Option<String>,
    pub unread_alert_count: i64,
}

impl Default for MarketSnapshot {
    fn default() -> Self {
        Self {
            status: "loading".to_string(),
            products: Vec::new(),
            shops: Vec::new(),
            protection: MarketProtection {
                mode: "normal".to_string(),
                active_api_base: "https://www.ldxp.cn".to_string(),
                data_mode: "empty".to_string(),
                ..MarketProtection::default()
            },
            last_checked_at: None,
            next_refresh_at: None,
            unread_alert_count: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketCategoryMetric {
    pub key: String,
    pub label: String,
    pub total_stock: i64,
    pub weighted_average_price: f64,
    pub minimum_price: f64,
    pub product_count: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAnalyticsPoint {
    pub captured_at: String,
    pub total_stock: i64,
    pub product_count: i64,
    pub categories: Vec<MarketCategoryMetric>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketEvent {
    pub seq: i64,
    pub event_id: String,
    pub kind: String,
    pub entity_type: String,
    pub entity_id: String,
    pub occurred_at: String,
    pub expires_at: Option<String>,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub section: String,
    pub payload: serde_json::Value,
    pub read_at: Option<String>,
    pub notified_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAnalyticsSnapshot {
    pub range: String,
    pub generated_at: String,
    pub points: Vec<MarketAnalyticsPoint>,
    pub events: Vec<MarketEvent>,
    pub total_samples: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketAlertSettings {
    pub enabled: bool,
    pub native_enabled: bool,
    pub bug_team_available: bool,
    pub product_unavailable: bool,
    pub store_health: bool,
    pub stock_surge: bool,
    pub price_outlier: bool,
    pub quiet_hours_enabled: bool,
    pub quiet_hours_start: String,
    pub quiet_hours_end: String,
}

impl Default for MarketAlertSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            native_enabled: true,
            bug_team_available: true,
            product_unavailable: false,
            store_health: true,
            stock_surge: false,
            price_outlier: false,
            quiet_hours_enabled: false,
            quiet_hours_start: "23:00".to_string(),
            quiet_hours_end: "08:00".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketShopInput {
    pub token: String,
    pub fallback_name: String,
    #[serde(default = "default_platform")]
    pub platform: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_platform() -> String {
    "liandx".to_string()
}

fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketRefreshResult {
    pub performed: bool,
    pub reason: Option<String>,
    pub retry_at: Option<String>,
    pub message: Option<String>,
    pub snapshot: MarketSnapshot,
}
