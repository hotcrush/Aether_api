use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub account_type: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    #[serde(skip_serializing)]
    pub access_token: String,
    #[serde(skip_serializing)]
    pub refresh_token: String,
    pub refreshable: bool,
    #[serde(skip_serializing)]
    pub id_token: String,
    #[serde(skip_serializing)]
    pub client_id: String,
    pub credential_masked: String,
    pub base_url: String,
    pub chatgpt_account_id: String,
    pub chatgpt_user_id: String,
    pub email: String,
    pub plan_type: String,
    pub expires_at: Option<i64>,
    pub priority: i64,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default = "default_weight")]
    pub weight: i64,
    pub concurrency: i64,
    pub status: String,
    pub last_error: String,
    pub last_used_at: Option<String>,
    pub request_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct NewAccount {
    pub name: String,
    pub account_type: String,
    pub api_key: String,
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub client_id: String,
    pub base_url: String,
    pub chatgpt_account_id: String,
    pub chatgpt_user_id: String,
    pub email: String,
    pub plan_type: String,
    pub expires_at: Option<i64>,
    pub priority: Option<i64>,
    /// `None` keeps the existing value during upsert; an empty list allows every model.
    pub models: Option<Vec<String>>,
    pub weight: Option<i64>,
    pub concurrency: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertAction {
    Created,
    Updated,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageTotals {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub unpriced_tokens: i64,
    pub total_cost: f64,
}

pub(super) const fn default_weight() -> i64 {
    1
}
