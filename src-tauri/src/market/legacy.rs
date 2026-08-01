use super::classifier::{classify_product, match_terms};
use super::types::{
    MarketAnalyticsPoint, MarketCategoryMetric, MarketEvent, MarketProduct, MarketProtection,
    MarketShop,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::Path;

pub struct LegacyMarketImport {
    pub products: Vec<MarketProduct>,
    pub shops: Vec<MarketShop>,
    pub protection: MarketProtection,
    pub last_checked_at: Option<String>,
    pub next_refresh_at: Option<String>,
    pub points: Vec<MarketAnalyticsPoint>,
    pub events: Vec<MarketEvent>,
}

pub fn load_legacy_market(data_dir: &Path) -> Result<Option<LegacyMarketImport>, String> {
    let watch_path = data_dir.join("product-watch.json");
    let analytics_path = data_dir.join("product-analytics.json");
    if !watch_path.is_file() && !analytics_path.is_file() {
        return Ok(None);
    }

    let watch = read_json(&watch_path)?.unwrap_or(Value::Null);
    let analytics = read_json(&analytics_path)?.unwrap_or(Value::Null);
    let last_checked_at = text(&watch, "lastCheckedAt");
    let next_refresh_at = text(&watch, "nextRefreshAt");
    let products = watch
        .get("allItems")
        .or_else(|| watch.get("items"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| legacy_product(value, last_checked_at.as_deref()))
        .collect::<Vec<_>>();
    let shops = watch
        .get("stores")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(legacy_shop)
        .collect::<Vec<_>>();
    let protection = legacy_protection(&watch, &shops, &products, last_checked_at.as_deref());
    let points = legacy_points(&analytics);
    let events = legacy_events(&analytics);

    Ok(Some(LegacyMarketImport {
        products,
        shops,
        protection,
        last_checked_at,
        next_refresh_at,
        points,
        events,
    }))
}

fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(value) => serde_json::from_str(&value)
            .map(Some)
            .map_err(|error| format!("读取旧市场数据失败 ({}): {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("读取旧市场数据失败 ({}): {error}", path.display())),
    }
}

fn legacy_product(value: &Value, checked_at: Option<&str>) -> Option<MarketProduct> {
    let id = text(value, "id")?;
    let name = text(value, "name")?;
    let shop_token = text(value, "shopToken")?;
    let shop_name = text(value, "shopName").unwrap_or_else(|| shop_token.clone());
    let goods_key = text(value, "goodsKey")
        .or_else(|| id.split_once(':').map(|(_, key)| key.to_string()))
        .unwrap_or_else(|| id.clone());
    let price = number(value, "price").unwrap_or_default().max(0.0);
    let fee = number(value, "fee").unwrap_or_default().max(0.0);
    let total_price = number(value, "totalPrice").unwrap_or(price + fee).max(0.0);
    let seen_at = checked_at
        .map(str::to_string)
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    Some(MarketProduct {
        id,
        goods_key,
        shop_token: shop_token.clone(),
        shop_name,
        shop_url: text(value, "shopUrl")
            .unwrap_or_else(|| format!("https://pay.ldxp.cn/shop/{shop_token}")),
        name: name.clone(),
        url: text(value, "url").unwrap_or_default(),
        price,
        fee,
        fee_rate: number(value, "feeRate").unwrap_or_default(),
        fee_payer: integer(value, "feePayer").unwrap_or_default(),
        total_price,
        market_price: number(value, "marketPrice").unwrap_or_default(),
        stock_count: integer(value, "stockCount").unwrap_or_default().max(0),
        source_category: text(value, "category").unwrap_or_default(),
        category: classify_product(&name).map(str::to_string),
        match_terms: match_terms(&name),
        verification_status: text(value, "verificationStatus")
            .unwrap_or_else(|| "unknown".to_string()),
        missing_count: 0,
        first_seen_at: seen_at.clone(),
        last_seen_at: seen_at,
    })
}

fn legacy_shop(value: &Value) -> Option<MarketShop> {
    let token = text(value, "token")?;
    let name = text(value, "name").unwrap_or_else(|| token.clone());
    let ok = boolean(value, "ok").unwrap_or(false);
    let last_checked_at = text(value, "lastCheckedAt");
    Some(MarketShop {
        platform: "liandx".to_string(),
        token,
        fallback_name: name.clone(),
        name,
        enabled: true,
        ok,
        error: text(value, "error"),
        failure_count: integer(value, "failureCount").unwrap_or_default(),
        blocked_until: text(value, "blockedUntil"),
        fee_rate: number(value, "feeRate").unwrap_or_default(),
        fee_payer: integer(value, "feePayer").unwrap_or_default(),
        fee_checked_at: text(value, "feeCheckedAt"),
        profile_checked_at: text(value, "profileCheckedAt"),
        last_checked_at: last_checked_at.clone(),
        last_success_at: ok.then_some(last_checked_at).flatten(),
        goods_types: strings(value.get("goodsTypes")),
        product_count: 0,
        total_stock: 0,
    })
}

fn legacy_protection(
    watch: &Value,
    shops: &[MarketShop],
    products: &[MarketProduct],
    checked_at: Option<&str>,
) -> MarketProtection {
    let guard = watch.get("_requestGuard").unwrap_or(&Value::Null);
    let consecutive_failures = integer(guard, "consecutiveFailures").unwrap_or_default();
    let circuit_open_until = text(guard, "circuitOpenUntil");
    let circuit_open = circuit_open_until
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_some_and(|value| value.with_timezone(&Utc) > Utc::now());
    let data_mode = if products.is_empty() {
        "empty"
    } else if shops.iter().any(|shop| !shop.ok) {
        "cached"
    } else {
        "live"
    };
    MarketProtection {
        mode: if circuit_open {
            "circuit-open"
        } else if consecutive_failures > 0 {
            "backoff"
        } else {
            "normal"
        }
        .to_string(),
        consecutive_failures,
        circuit_open_until,
        circuit_reason: text(guard, "circuitReason"),
        last_attempt_at: text(guard, "lastAttemptAt"),
        last_request_at: text(guard, "lastRequestAt"),
        last_success_at: text(guard, "lastSuccessAt").or_else(|| checked_at.map(str::to_string)),
        request_timestamps: strings(guard.get("requestTimestamps")),
        active_api_base: text(guard, "activeApiBase")
            .unwrap_or_else(|| "https://www.ldxp.cn".to_string()),
        fallback_used: boolean(guard, "fallbackUsed").unwrap_or(false),
        data_mode: data_mode.to_string(),
    }
}

fn legacy_points(analytics: &Value) -> Vec<MarketAnalyticsPoint> {
    analytics
        .get("snapshots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|point| {
            let captured_at = text(point, "capturedAt")?;
            let categories = point
                .get("categories")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|category| {
                    let key = text(category, "key")?;
                    let weighted_average_price =
                        number(category, "weightedAveragePrice").unwrap_or_default();
                    Some(MarketCategoryMetric {
                        label: text(category, "label").unwrap_or_else(|| key.clone()),
                        key,
                        total_stock: integer(category, "totalStock").unwrap_or_default(),
                        weighted_average_price,
                        minimum_price: number(category, "minimumPrice")
                            .unwrap_or(weighted_average_price),
                        product_count: integer(category, "productCount").unwrap_or_default(),
                    })
                })
                .collect();
            Some(MarketAnalyticsPoint {
                captured_at,
                total_stock: integer(point, "totalStock").unwrap_or_default(),
                product_count: integer(point, "productCount").unwrap_or_default(),
                categories,
            })
        })
        .collect()
}

fn legacy_events(analytics: &Value) -> Vec<MarketEvent> {
    let mut events = Vec::new();
    for value in array(analytics, "zeroEvents") {
        let Some(occurred_at) = text(value, "detectedAt") else {
            continue;
        };
        let entity_id = text(value, "productId").unwrap_or_default();
        let product_name = text(value, "productName").unwrap_or_else(|| "未知商品".to_string());
        let shop_name = text(value, "shopName").unwrap_or_else(|| "未知店铺".to_string());
        let previous_stock = integer(value, "previousStock").unwrap_or_default();
        events.push(legacy_event(
            format!(
                "legacy:zero:{}",
                text(value, "id").unwrap_or_else(|| entity_id.clone())
            ),
            "product.unavailable",
            "product",
            entity_id,
            occurred_at,
            "warning",
            "商品历史缺货",
            format!("{product_name} · {shop_name} · 原库存 {previous_stock}"),
            "products",
            value.clone(),
        ));
    }
    for value in array(analytics, "stockSurges") {
        let Some(occurred_at) = text(value, "detectedAt") else {
            continue;
        };
        let increase = integer(value, "increase").unwrap_or_default();
        let current_stock = integer(value, "currentStock").unwrap_or_default();
        events.push(legacy_event(
            format!(
                "legacy:{}",
                text(value, "id").unwrap_or_else(|| occurred_at.clone())
            ),
            "market.stock_surge",
            "market",
            "market".to_string(),
            occurred_at,
            text(value, "severity").as_deref().unwrap_or("medium"),
            "市场历史集中补库",
            format!("总库存增加 {increase} 件，当时库存 {current_stock} 件"),
            "analytics",
            value.clone(),
        ));
    }
    for value in array(analytics, "behaviorEvents") {
        let Some(occurred_at) = text(value, "detectedAt") else {
            continue;
        };
        let category = text(value, "category").unwrap_or_else(|| "market".to_string());
        let label = text(value, "categoryLabel").unwrap_or_else(|| category.clone());
        let redistributed = integer(value, "redistributedStock").unwrap_or_default();
        events.push(legacy_event(
            format!(
                "legacy:{}",
                text(value, "id").unwrap_or_else(|| occurred_at.clone())
            ),
            "market.suspected_hoarding",
            "market",
            category,
            occurred_at,
            text(value, "severity").as_deref().unwrap_or("medium"),
            &format!("{label} 历史疑似扫货/库存转移"),
            format!("历史记录显示疑似转移 {redistributed} 件库存"),
            "analytics",
            value.clone(),
        ));
    }
    events
}

#[allow(clippy::too_many_arguments)]
fn legacy_event(
    event_id: String,
    kind: &str,
    entity_type: &str,
    entity_id: String,
    occurred_at: String,
    severity: &str,
    title: &str,
    body: String,
    section: &str,
    payload: Value,
) -> MarketEvent {
    MarketEvent {
        seq: 0,
        event_id,
        kind: kind.to_string(),
        entity_type: entity_type.to_string(),
        entity_id,
        read_at: Some(occurred_at.clone()),
        occurred_at,
        expires_at: None,
        severity: severity.to_string(),
        title: title.to_string(),
        body,
        section: section.to_string(),
        payload,
        notified_at: None,
    }
}

fn array<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|value| match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        _ => None,
    })
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn integer(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|value| value.round() as i64))
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn boolean(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
