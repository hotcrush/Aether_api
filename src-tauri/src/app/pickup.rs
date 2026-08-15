use super::accounts::import_parsed_accounts;
use super::AppState;
use crate::account_import::{self, ImportResult};
use reqwest::header::{HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PICKUP_BASE_URL: &str = "https://bugteam.team";
const PICKUP_TOKEN_SETTING: &str = "pickup_customer_token";
const PICKUP_ORDERS_SETTING: &str = "pickup_orders_v1";
const PICKUP_PRODUCT: &str = "team_1h";
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;
const MAX_SAVED_ORDERS: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PickupSettings {
    customer_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PickupImportMessage {
    index: usize,
    name: String,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PickupImportResult {
    total: usize,
    created: usize,
    updated: usize,
    failed: usize,
    errors: Vec<PickupImportMessage>,
}

impl From<ImportResult> for PickupImportResult {
    fn from(result: ImportResult) -> Self {
        Self {
            total: result.total,
            created: result.created,
            updated: result.updated,
            failed: result.failed,
            errors: result
                .errors
                .into_iter()
                .map(|error| PickupImportMessage {
                    index: error.index,
                    name: error.name,
                    message: error.message,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PickupOrderRecord {
    idempotency_key: String,
    order_id: String,
    product: String,
    quantity: u32,
    state: String,
    hold_total_fen: Option<i64>,
    charged_fen: Option<i64>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    response: Value,
    #[serde(default)]
    import_attempted_at: Option<String>,
    #[serde(default)]
    import_result: Option<PickupImportResult>,
    #[serde(default)]
    import_error: String,
    #[serde(default)]
    last_error: String,
}

#[tauri::command]
pub(super) fn get_pickup_settings(state: tauri::State<AppState>) -> Result<PickupSettings, String> {
    Ok(PickupSettings {
        customer_token: state
            .db
            .get_setting(PICKUP_TOKEN_SETTING)
            .map_err(|error| error.to_string())?
            .unwrap_or_default(),
    })
}

#[tauri::command]
pub(super) fn update_pickup_settings(
    state: tauri::State<AppState>,
    settings: PickupSettings,
) -> Result<PickupSettings, String> {
    let customer_token = settings.customer_token.trim().to_string();
    if customer_token.len() > 512 || customer_token.chars().any(char::is_whitespace) {
        return Err("Customer Token 格式无效".to_string());
    }
    state
        .db
        .set_setting(PICKUP_TOKEN_SETTING, &customer_token)
        .map_err(|error| error.to_string())?;
    Ok(PickupSettings { customer_token })
}

#[tauri::command]
pub(super) async fn get_pickup_overview(
    state: tauri::State<'_, AppState>,
    quantity: u32,
) -> Result<Value, String> {
    validate_quantity(quantity)?;
    let token = customer_token(&state)?;
    let client = state.client.load_full();
    let inventory_path =
        format!("/api/customer/inventory?product={PICKUP_PRODUCT}&quantity={quantity}");
    let balance = request_json(
        &client,
        Method::GET,
        "/api/customer/balance",
        &token,
        None,
        None,
    )
    .await?;
    let inventory = request_json(&client, Method::GET, &inventory_path, &token, None, None).await?;
    Ok(json!({ "balance": balance, "inventory": inventory }))
}

#[tauri::command]
pub(super) async fn list_pickup_orders(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<PickupOrderRecord>, String> {
    let _guard = state.pickup_orders.lock().await;
    load_orders(&state)
}

#[tauri::command]
pub(super) async fn create_pickup_order(
    state: tauri::State<'_, AppState>,
    quantity: u32,
    idempotency_key: String,
) -> Result<PickupOrderRecord, String> {
    validate_quantity(quantity)?;
    validate_idempotency_key(&idempotency_key)?;

    let now = now_string();
    let mut record = {
        let _guard = state.pickup_orders.lock().await;
        let mut orders = load_orders(&state)?;
        if let Some(existing) = orders
            .iter()
            .find(|order| order.idempotency_key == idempotency_key && !order.order_id.is_empty())
        {
            return Ok(existing.clone());
        }
        let record = orders
            .iter()
            .find(|order| order.idempotency_key == idempotency_key)
            .cloned()
            .unwrap_or_else(|| PickupOrderRecord {
                idempotency_key: idempotency_key.clone(),
                order_id: String::new(),
                product: PICKUP_PRODUCT.to_string(),
                quantity,
                state: "submitting".to_string(),
                hold_total_fen: None,
                charged_fen: None,
                created_at: now.clone(),
                updated_at: now.clone(),
                response: Value::Null,
                import_attempted_at: None,
                import_result: None,
                import_error: String::new(),
                last_error: String::new(),
            });
        upsert_order(&mut orders, record.clone());
        save_orders(&state, &orders)?;
        record
    };

    let token = customer_token(&state)?;
    let client = state.client.load_full();
    let payload = json!({ "product": PICKUP_PRODUCT, "quantity": quantity });
    let response = match request_json(
        &client,
        Method::POST,
        "/api/customer/pickup/orders",
        &token,
        Some(payload),
        Some(&idempotency_key),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            record.state = "submit_unknown".to_string();
            record.updated_at = now_string();
            record.last_error = error.clone();
            persist_order(&state, record).await?;
            return Err(format!("{error}；再次提交会复用同一幂等键"));
        }
    };

    let order_id = order_string(&response, "order_id")
        .or_else(|| order_string(&response, "id"))
        .ok_or_else(|| "取号服务未返回 order_id".to_string())?;
    validate_order_id(&order_id)?;
    record.order_id = order_id;
    apply_order_response(&mut record, response);
    persist_order(&state, record.clone()).await?;
    maybe_import_completed_order(&state, record, false).await
}

#[tauri::command]
pub(super) async fn refresh_pickup_order(
    state: tauri::State<'_, AppState>,
    order_id: String,
) -> Result<PickupOrderRecord, String> {
    validate_order_id(&order_id)?;
    let token = customer_token(&state)?;
    let client = state.client.load_full();
    let response = request_json(
        &client,
        Method::GET,
        &format!("/api/customer/pickup/orders/{order_id}"),
        &token,
        None,
        None,
    )
    .await?;

    let mut record = {
        let _guard = state.pickup_orders.lock().await;
        load_orders(&state)?
            .into_iter()
            .find(|order| order.order_id == order_id)
            .ok_or_else(|| "本地订单记录不存在".to_string())?
    };
    apply_order_response(&mut record, response);
    persist_order(&state, record.clone()).await?;
    maybe_import_completed_order(&state, record, false).await
}

#[tauri::command]
pub(super) async fn retry_pickup_order_import(
    state: tauri::State<'_, AppState>,
    order_id: String,
) -> Result<PickupOrderRecord, String> {
    validate_order_id(&order_id)?;
    let mut record = {
        let _guard = state.pickup_orders.lock().await;
        load_orders(&state)?
            .into_iter()
            .find(|order| order.order_id == order_id)
            .ok_or_else(|| "本地订单记录不存在".to_string())?
    };
    if !is_completed(&record.state) {
        return Err("订单尚未完成，暂时不能下载导入".to_string());
    }
    record.import_attempted_at = None;
    record.import_result = None;
    record.import_error.clear();
    persist_order(&state, record.clone()).await?;
    maybe_import_completed_order(&state, record, true).await
}

async fn maybe_import_completed_order(
    state: &AppState,
    mut record: PickupOrderRecord,
    force: bool,
) -> Result<PickupOrderRecord, String> {
    if !is_completed(&record.state) || (!force && record.import_attempted_at.is_some()) {
        return Ok(record);
    }

    record.import_attempted_at = Some(now_string());
    record.import_error.clear();
    persist_order(state, record.clone()).await?;

    match download_and_import(state, &record.order_id).await {
        Ok(result) => record.import_result = Some(result),
        Err(error) => record.import_error = error,
    }
    record.updated_at = now_string();
    persist_order(state, record.clone()).await?;
    Ok(record)
}

async fn download_and_import(
    state: &AppState,
    order_id: &str,
) -> Result<PickupImportResult, String> {
    let token = customer_token(state)?;
    let client = state.client.load_full();
    let content = request_text(
        &client,
        Method::GET,
        &format!("/api/customer/pickup/orders/{order_id}/download?format=sub2"),
        &token,
        MAX_DOWNLOAD_BYTES,
    )
    .await?;
    let (accounts, parse_errors) = account_import::parse_import_contents(&[content]);
    Ok(import_parsed_accounts(state, accounts, parse_errors, 1)
        .await
        .into())
}

async fn request_json(
    client: &reqwest::Client,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
    idempotency_key: Option<&str>,
) -> Result<Value, String> {
    let mut request = client
        .request(method, format!("{PICKUP_BASE_URL}{path}"))
        .header("X-Customer-Token", token_header(token)?)
        .header(ACCEPT, "application/json");
    if let Some(idempotency_key) = idempotency_key {
        request = request.header("Idempotency-Key", idempotency_key);
    }
    if let Some(body) = body {
        request = request.header(CONTENT_TYPE, "application/json").json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("连接取号服务失败: {error}"))?;
    response_json(response, MAX_JSON_BYTES).await
}

async fn request_text(
    client: &reqwest::Client,
    method: Method,
    path: &str,
    token: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let response = client
        .request(method, format!("{PICKUP_BASE_URL}{path}"))
        .header("X-Customer-Token", token_header(token)?)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| format!("下载 Sub2 JSON 失败: {error}"))?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("读取取号服务响应失败: {error}"))?;
    if bytes.len() > max_bytes {
        return Err("下载的 Sub2 JSON 超过 10 MB".to_string());
    }
    if !status.is_success() {
        return Err(remote_error(status, &bytes, retry_after.as_deref()));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "下载内容不是 UTF-8 JSON".to_string())
}

async fn response_json(response: reqwest::Response, max_bytes: usize) -> Result<Value, String> {
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("读取取号服务响应失败: {error}"))?;
    if bytes.len() > max_bytes {
        return Err("取号服务响应过大".to_string());
    }
    if !status.is_success() {
        return Err(remote_error(status, &bytes, retry_after.as_deref()));
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("取号服务返回无效 JSON: {error}"))
}

fn remote_error(status: reqwest::StatusCode, bytes: &[u8], retry_after: Option<&str>) -> String {
    let value = serde_json::from_slice::<Value>(bytes).ok();
    let message = value
        .as_ref()
        .and_then(error_message)
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).trim().to_string());
    let message = if message.is_empty() {
        status.canonical_reason().unwrap_or("请求失败").to_string()
    } else {
        message
    };
    let retry = retry_after
        .map(|seconds| format!("，请在 {seconds} 秒后重试"))
        .unwrap_or_default();
    format!("取号服务 HTTP {}: {message}{retry}", status.as_u16())
}

fn error_message(value: &Value) -> Option<String> {
    for candidate in [
        value.get("message"),
        value.get("detail"),
        value.get("error").and_then(|error| error.get("message")),
        value.get("error"),
        value.get("code"),
    ] {
        if let Some(message) = candidate.and_then(Value::as_str) {
            if !message.trim().is_empty() {
                return Some(message.trim().to_string());
            }
        }
    }
    None
}

fn customer_token(state: &AppState) -> Result<String, String> {
    let token = state
        .db
        .get_setting(PICKUP_TOKEN_SETTING)
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    if token.trim().is_empty() {
        Err("请先在设置页填写 Customer Token".to_string())
    } else {
        Ok(token)
    }
}

fn token_header(token: &str) -> Result<HeaderValue, String> {
    HeaderValue::from_str(token).map_err(|_| "Customer Token 格式无效".to_string())
}

fn validate_quantity(quantity: u32) -> Result<(), String> {
    if (1..=1_000).contains(&quantity) {
        Ok(())
    } else {
        Err("取号数量必须在 1 到 1000 之间".to_string())
    }
}

fn validate_idempotency_key(value: &str) -> Result<(), String> {
    if value.len() >= 16
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err("幂等键格式无效".to_string())
    }
}

fn validate_order_id(value: &str) -> Result<(), String> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err("订单 ID 格式无效".to_string())
    }
}

fn apply_order_response(record: &mut PickupOrderRecord, response: Value) {
    if let Some(state) =
        order_string(&response, "state").or_else(|| order_string(&response, "status"))
    {
        record.state = state;
    } else if record.state == "submitting" || record.state == "submit_unknown" {
        record.state = "created".to_string();
    }
    record.hold_total_fen = order_i64(&response, "hold_total_fen").or(record.hold_total_fen);
    record.charged_fen = order_i64(&response, "charged_fen").or(record.charged_fen);
    record.response = response;
    record.last_error.clear();
    record.updated_at = now_string();
}

fn order_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value
        .get(key)
        .or_else(|| value.get("order").and_then(|order| order.get(key)))
        .or_else(|| value.get("data").and_then(|data| data.get(key)))
}

fn order_string(value: &Value, key: &str) -> Option<String> {
    order_value(value, key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn order_i64(value: &Value, key: &str) -> Option<i64> {
    order_value(value, key).and_then(Value::as_i64)
}

fn is_completed(state: &str) -> bool {
    matches!(
        state.trim().to_ascii_lowercase().as_str(),
        "completed" | "complete" | "fulfilled" | "delivered" | "success"
    )
}

async fn persist_order(state: &AppState, record: PickupOrderRecord) -> Result<(), String> {
    let _guard = state.pickup_orders.lock().await;
    let mut orders = load_orders(state)?;
    upsert_order(&mut orders, record);
    save_orders(state, &orders)
}

fn load_orders(state: &AppState) -> Result<Vec<PickupOrderRecord>, String> {
    let Some(raw) = state
        .db
        .get_setting(PICKUP_ORDERS_SETTING)
        .map_err(|error| error.to_string())?
    else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&raw).map_err(|error| format!("本地取号订单记录损坏: {error}"))
}

fn save_orders(state: &AppState, orders: &[PickupOrderRecord]) -> Result<(), String> {
    let value = serde_json::to_string(orders).map_err(|error| error.to_string())?;
    state
        .db
        .set_setting(PICKUP_ORDERS_SETTING, &value)
        .map_err(|error| error.to_string())
}

fn upsert_order(orders: &mut Vec<PickupOrderRecord>, record: PickupOrderRecord) {
    orders.retain(|order| {
        order.idempotency_key != record.idempotency_key
            && (record.order_id.is_empty() || order.order_id != record.order_id)
    });
    orders.insert(0, record);
    orders.truncate(MAX_SAVED_ORDERS);
}

fn now_string() -> String {
    chrono::Utc::now().to_rfc3339()
}
