use super::classifier::{
    category_metrics, classify_product, match_terms, price_profiles, verification_status,
};
use super::database::MarketDatabase;
use super::types::{
    MarketAlertSettings, MarketAnalyticsPoint, MarketAnalyticsSnapshot, MarketEvent, MarketProduct,
    MarketProtection, MarketRefreshResult, MarketShop, MarketShopInput, MarketSnapshot,
};
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveTime, Utc};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, ORIGIN, REFERER, USER_AGENT,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, RwLock};

const API_BASES: [&str; 2] = ["https://www.ldxp.cn", "https://pay.ldxp.cn"];
const SHOP_BASE: &str = "https://pay.ldxp.cn";
const REFRESH_SECONDS: i64 = 90;
const REQUEST_SPACING_MS: i64 = 300;
const MANUAL_COOLDOWN_SECONDS: i64 = 30;
const PROFILE_CACHE_SECONDS: i64 = 60 * 60;
const FEE_CACHE_SECONDS: i64 = 6 * 60 * 60;
const MAX_BACKOFF_SECONDS: i64 = 30 * 60;
const BOOTSTRAP_RECOVERY_SECONDS: i64 = 20;
const PAGE_SIZE: i64 = 200;
const SHOP_CONCURRENCY: usize = 3;

#[derive(Debug)]
pub struct MarketState {
    database: Arc<MarketDatabase>,
    client: reqwest::Client,
    app: AppHandle,
    snapshot: RwLock<MarketSnapshot>,
    refresh_gate: Mutex<()>,
    protection_checkpoint: Mutex<Option<Instant>>,
}

impl MarketState {
    pub fn new(
        path: impl AsRef<std::path::Path>,
        client: reqwest::Client,
        app: AppHandle,
        legacy_data_dir: Option<&std::path::Path>,
    ) -> Result<Arc<Self>, String> {
        let database = Arc::new(MarketDatabase::new(path)?);
        if let Some(data_dir) = legacy_data_dir {
            match database.import_legacy(data_dir) {
                Ok(true) => tracing::info!(path = %data_dir.display(), "已导入旧版市场监控数据"),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(%error, path = %data_dir.display(), "旧版市场数据导入失败")
                }
            }
        }
        let snapshot = database.load_snapshot()?;
        Ok(Arc::new(Self {
            database,
            client,
            app,
            snapshot: RwLock::new(snapshot),
            refresh_gate: Mutex::new(()),
            protection_checkpoint: Mutex::new(None),
        }))
    }

    pub fn start(state: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            loop {
                let wait = {
                    let snapshot = state.snapshot.read().await;
                    snapshot
                        .next_refresh_at
                        .as_deref()
                        .and_then(parse_time)
                        .map(|target| (target - Utc::now()).to_std().unwrap_or_default())
                        .unwrap_or_default()
                };
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                match state.refresh(false).await {
                    Ok(result) if !result.performed => {
                        let retry_wait = result
                            .retry_at
                            .as_deref()
                            .and_then(parse_time)
                            .map(|target| (target - Utc::now()).to_std().unwrap_or_default())
                            .unwrap_or_else(|| Duration::from_secs(1));
                        tokio::time::sleep(retry_wait.max(Duration::from_secs(1))).await;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, "市场监控刷新失败");
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                }
            }
        });
    }

    pub async fn current_snapshot(&self) -> MarketSnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn refresh(&self, manual: bool) -> Result<MarketRefreshResult, String> {
        let _guard = match self.refresh_gate.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                return Ok(MarketRefreshResult {
                    performed: false,
                    reason: Some("inflight".to_string()),
                    retry_at: self.snapshot.read().await.next_refresh_at.clone(),
                    message: Some("已有市场刷新正在进行".to_string()),
                    snapshot: self.current_snapshot().await,
                });
            }
        };

        let previous = self.current_snapshot().await;
        let now = Utc::now();
        if let Some(circuit_until) = previous
            .protection
            .circuit_open_until
            .as_deref()
            .and_then(parse_time)
        {
            if circuit_until > now {
                let mut result = deferred_result("circuit-open", "接口保护冷却中", &previous);
                result.retry_at = Some(circuit_until.to_rfc3339());
                return Ok(result);
            }
        }
        let has_usable_snapshot = !previous.products.is_empty();
        if manual && has_usable_snapshot {
            if let Some(last_attempt) = previous
                .protection
                .last_attempt_at
                .as_deref()
                .and_then(parse_time)
            {
                let retry_at = last_attempt + ChronoDuration::seconds(MANUAL_COOLDOWN_SECONDS);
                if retry_at > now {
                    let mut result =
                        deferred_result("cooldown", "手动刷新仍在三分钟冷却期", &previous);
                    result.retry_at = Some(retry_at.to_rfc3339());
                    return Ok(result);
                }
            }
        }

        let mut protection = previous.protection.clone();
        if protection
            .circuit_open_until
            .as_deref()
            .and_then(parse_time)
            .is_some_and(|until| until <= now)
        {
            protection.circuit_open_until = None;
            protection.circuit_reason = None;
        }
        protection.last_attempt_at = Some(now.to_rfc3339());
        protection.mode = "normal".to_string();
        protection.request_timestamps.retain(|value| {
            parse_time(value)
                .map(|time| time > now - ChronoDuration::hours(1))
                .unwrap_or(false)
        });

        let shared_protection = Arc::new(tokio::sync::Mutex::new(protection.clone()));

        let previous_by_id = previous
            .products
            .iter()
            .cloned()
            .map(|product| (product.id.clone(), product))
            .collect::<HashMap<_, _>>();
        let previous_by_shop = previous.products.iter().cloned().fold(
            HashMap::<String, Vec<MarketProduct>>::new(),
            |mut groups, item| {
                groups
                    .entry(item.shop_token.clone())
                    .or_default()
                    .push(item);
                groups
            },
        );
        let baseline = protection.last_success_at.is_none();
        let mut products = Vec::new();
        let mut shops = Vec::new();
        let mut successful_tokens = HashSet::new();
        let attempted_store_count;
        let mut events = Vec::new();

        // Separate shops into disabled/blocked (sequential) and fetchable (parallel)
        let mut fetchable: Vec<MarketShop> = Vec::new();
        for configured in previous.shops.clone() {
            if !configured.enabled {
                let mut disabled = configured;
                disabled.ok = false;
                disabled.error = None;
                disabled.product_count = 0;
                disabled.total_stock = 0;
                shops.push(disabled);
                continue;
            }
            if let Some(domain_blocked_until) = protection
                .circuit_open_until
                .as_deref()
                .and_then(parse_time)
                .filter(|until| *until > Utc::now())
            {
                let mut blocked = configured;
                blocked.ok = false;
                blocked.error = protection
                    .circuit_reason
                    .clone()
                    .or_else(|| Some("接口保护冷却中".to_string()));
                let existing_blocked_until = blocked.blocked_until.as_deref().and_then(parse_time);
                if existing_blocked_until.is_none_or(|until| until < domain_blocked_until) {
                    blocked.blocked_until = Some(domain_blocked_until.to_rfc3339());
                }
                products.extend(
                    previous_by_shop
                        .get(&blocked.token)
                        .cloned()
                        .unwrap_or_default(),
                );
                shops.push(blocked);
                continue;
            }
            if configured
                .blocked_until
                .as_deref()
                .and_then(parse_time)
                .is_some_and(|until| until > Utc::now())
            {
                products.extend(
                    previous_by_shop
                        .get(&configured.token)
                        .cloned()
                        .unwrap_or_default(),
                );
                shops.push(configured);
                continue;
            }
            fetchable.push(configured);
        }

        // Fetch shops concurrently with bounded parallelism
        attempted_store_count = fetchable.len();
        {
            use futures::stream::{self, StreamExt};
            let results: Vec<(MarketShop, Result<(MarketShop, Vec<MarketProduct>), String>)> =
                stream::iter(fetchable.into_iter().map(|configured| {
                    let state = self;
                    let prot = Arc::clone(&shared_protection);
                    async move {
                        let result = state.fetch_shop(configured.clone(), &prot).await;
                        (configured, result)
                    }
                }))
                .buffer_unordered(SHOP_CONCURRENCY)
                .collect()
                .await;

            for (configured, result) in results {
                match result {
                    Ok((mut shop, fetched)) => {
                        successful_tokens.insert(shop.token.clone());
                        let fetched = fetched
                            .into_iter()
                            .map(|mut item| {
                                if let Some(old) = previous_by_id.get(&item.id) {
                                    item.first_seen_at = old.first_seen_at.clone();
                                }
                                item
                            })
                            .collect::<Vec<_>>();
                        let fetched_ids = fetched
                            .iter()
                            .map(|item| item.id.clone())
                            .collect::<HashSet<_>>();
                        products.extend(fetched);
                        for mut old in previous_by_shop
                            .get(&shop.token)
                            .cloned()
                            .unwrap_or_default()
                        {
                            if fetched_ids.contains(&old.id) {
                                continue;
                            }
                            old.missing_count += 1;
                            if old.missing_count < 2 {
                                products.push(old);
                            } else {
                                events.push(product_event(
                                    "product.unavailable",
                                    &old,
                                    "商品确认缺货",
                                    format!(
                                        "{} · {} · 原库存 {}",
                                        old.name, old.shop_name, old.stock_count
                                    ),
                                    "warning",
                                ));
                            }
                        }
                        let shop_products =
                            products.iter().filter(|item| item.shop_token == shop.token);
                        shop.product_count = shop_products.clone().count() as i64;
                        shop.total_stock = shop_products.map(|item| item.stock_count).sum();
                        if configured.failure_count >= 3 {
                            events.push(store_event(
                                "store.recovered",
                                &shop,
                                "店铺已恢复",
                                format!("{} 已恢复正常采集", shop.name),
                                "success",
                            ));
                        }
                        shops.push(shop);
                    }
                    Err(error) => {
                        let mut failed = configured.clone();
                        failed.ok = false;
                        failed.failure_count += 1;
                        failed.error = Some(error.chars().take(180).collect());
                        let backoff = if has_usable_snapshot {
                            (REFRESH_SECONDS * 2_i64.pow((failed.failure_count - 1).clamp(0, 5) as u32))
                                .min(MAX_BACKOFF_SECONDS)
                        } else {
                            BOOTSTRAP_RECOVERY_SECONDS
                        };
                        failed.blocked_until =
                            Some((Utc::now() + ChronoDuration::seconds(backoff)).to_rfc3339());
                        products.extend(
                            previous_by_shop
                                .get(&failed.token)
                                .cloned()
                                .unwrap_or_default(),
                        );
                        if failed.failure_count == 3 {
                            events.push(store_event(
                                "store.degraded",
                                &failed,
                                "店铺连续采集失败",
                                format!(
                                    "{} · {}",
                                    failed.name,
                                    failed.error.clone().unwrap_or_default()
                                ),
                                "high",
                            ));
                        }
                        shops.push(failed);
                    }
                }
            }
        }

        // Extract final protection state
        let mut protection = shared_protection.lock().await.clone();

        products.sort_by(|a, b| {
            a.total_price
                .total_cmp(&b.total_price)
                .then_with(|| a.shop_name.cmp(&b.shop_name))
        });
        products.dedup_by(|a, b| a.id == b.id);
        if !baseline {
            for product in &products {
                // Only generate arrival events for focus categories (K12, GPT Plus, BUG TEAM)
                let is_focus = product.category.as_deref().is_some_and(|c| c == "k12" || c == "gptplus" || c == "bugteam");
                if !previous_by_id.contains_key(&product.id) && product.missing_count == 0 && is_focus {
                    events.push(product_event(
                        "product.available",
                        product,
                        match product.category.as_deref() {
                            Some("bugteam") => "BUG TEAM 到货",
                            Some("k12") => "K12 到货",
                            Some("gptplus") => "GPT Plus 到货",
                            _ => "商品到货",
                        },
                        format!(
                            "{} · {} · 库存 {} · 到手价 ¥{}",
                            product.name,
                            product.shop_name,
                            product.stock_count,
                            format_money(product.total_price)
                        ),
                        if product.category.as_deref() == Some("bugteam") {
                            "high"
                        } else {
                            "info"
                        },
                    ));
                }
            }
        }

        let success_count = successful_tokens.len();
        if success_count > 0 {
            protection.consecutive_failures = 0;
            protection.last_success_at = Some(Utc::now().to_rfc3339());
        } else if attempted_store_count > 0 {
            protection.consecutive_failures += 1;
            let circuit_active = protection
                .circuit_open_until
                .as_deref()
                .and_then(parse_time)
                .is_some_and(|until| until > Utc::now());
            if !circuit_active && protection.consecutive_failures >= 3 && has_usable_snapshot {
                protection.circuit_open_until =
                    Some((Utc::now() + ChronoDuration::minutes(10)).to_rfc3339());
                protection.circuit_reason = Some("连续刷新失败".to_string());
            }
        }
        let circuit_active = protection
            .circuit_open_until
            .as_deref()
            .and_then(parse_time)
            .is_some_and(|until| until > Utc::now());
        if circuit_active {
            protection.mode = "circuit-open".to_string();
        } else {
            protection.circuit_open_until = None;
            protection.circuit_reason = None;
            protection.mode = if protection.consecutive_failures > 0 {
                "backoff"
            } else {
                "normal"
            }
            .to_string();
        }
        protection.data_mode = if products.is_empty() {
            "empty"
        } else if success_count < shops.iter().filter(|shop| shop.enabled).count() {
            "cached"
        } else {
            "live"
        }
        .to_string();

        add_market_signal_events(&previous.products, &products, &mut events);
        let captured_at = protection
            .last_success_at
            .clone()
            .or_else(|| previous.last_checked_at.clone())
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        let categories = category_metrics(&products);
        let point = MarketAnalyticsPoint {
            captured_at: captured_at.clone(),
            total_stock: categories.iter().map(|category| category.total_stock).sum(),
            product_count: categories
                .iter()
                .map(|category| category.product_count)
                .sum(),
            categories,
        };
        let enabled_shops = shops.iter().filter(|shop| shop.enabled).collect::<Vec<_>>();
        let status = if enabled_shops.is_empty() {
            "idle"
        } else if enabled_shops.iter().all(|shop| shop.ok) {
            "online"
        } else if enabled_shops.iter().any(|shop| shop.ok) {
            "partial"
        } else {
            "error"
        };
        let base_next_seconds = if success_count > 0 {
            REFRESH_SECONDS
        } else if !has_usable_snapshot {
            BOOTSTRAP_RECOVERY_SECONDS
        } else {
            (REFRESH_SECONDS * 2_i64.pow(protection.consecutive_failures.clamp(0, 4) as u32))
                .min(MAX_BACKOFF_SECONDS)
        };
        let circuit_wait_seconds = protection
            .circuit_open_until
            .as_deref()
            .and_then(parse_time)
            .map(|until| (until - Utc::now()).num_seconds().max(0))
            .unwrap_or_default();
        let next_seconds = base_next_seconds.max(circuit_wait_seconds);
        let mut snapshot = MarketSnapshot {
            status: status.to_string(),
            products,
            shops,
            protection,
            last_checked_at: Some(captured_at),
            next_refresh_at: Some(
                (Utc::now() + ChronoDuration::seconds(next_seconds)).to_rfc3339(),
            ),
            unread_alert_count: previous.unread_alert_count,
        };
        let inserted = self.database.persist_refresh(
            &snapshot,
            (success_count > 0).then_some(&point),
            &events,
        )?;
        snapshot = self.database.load_snapshot()?;
        *self.snapshot.write().await = snapshot.clone();
        let _ = self.app.emit("market:snapshot", &snapshot);
        for event in inserted {
            let _ = self.app.emit("market:alert", &event);
            self.deliver_notification(&event).await;
        }
        Ok(MarketRefreshResult {
            performed: true,
            reason: None,
            retry_at: None,
            message: None,
            snapshot,
        })
    }

    async fn fetch_shop(
        &self,
        mut shop: MarketShop,
        protection: &Arc<tokio::sync::Mutex<MarketProtection>>,
    ) -> Result<(MarketShop, Vec<MarketProduct>), String> {
        let now = Utc::now();
        let previous_fee_payer = shop.fee_payer;
        let profile_expired = shop.goods_types.is_empty()
            || shop
                .profile_checked_at
                .as_deref()
                .and_then(parse_time)
                .is_none_or(|checked| {
                    checked < now - ChronoDuration::seconds(PROFILE_CACHE_SECONDS)
                });
        if profile_expired {
            let existing_goods_types = shop.goods_types.clone();
            let info = match self
                .post_json(
                    "/shopApi/Shop/info",
                    json!({ "token": shop.token, "category_key": "" }),
                    protection,
                    !existing_goods_types.is_empty(),
                )
                .await
            {
                Ok(info) => Some(info),
                Err(_) if !existing_goods_types.is_empty() => None,
                Err(error) => return Err(error),
            };
            if let Some(info) = info {
                shop.name =
                    string_value(&info["nickname"]).unwrap_or_else(|| shop.fallback_name.clone());
                shop.fee_payer = integer_value(&info["params"]["default_fee_payer"]);
                shop.goods_types = info["goods_type_sort"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(string_value)
                            .filter(|kind| {
                                let count_key = format!("{kind}_count");
                                integer_value(info.get(&count_key).unwrap_or(&Value::Null)) > 0
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["card".to_string()]);
                shop.profile_checked_at = Some(now.to_rfc3339());
            }
        }

        let fee_expired = shop
            .fee_checked_at
            .as_deref()
            .and_then(parse_time)
            .is_none_or(|checked| checked < now - ChronoDuration::seconds(FEE_CACHE_SECONDS))
            || previous_fee_payer != shop.fee_payer;
        if shop.fee_payer == 1 && fee_expired {
            if let Ok(channels) = self
                .post_json(
                    "/shopApi/Shop/getUserChannel",
                    json!({ "token": shop.token }),
                    protection,
                    true,
                )
                .await
            {
                shop.fee_rate = channels
                    .as_array()
                    .and_then(|items| {
                        items.iter().find(|channel| {
                            channel
                                .get("status")
                                .filter(|value| !value.is_null())
                                .map(integer_value)
                                .unwrap_or(1)
                                != 0
                                && channel
                                    .get("custom_status")
                                    .filter(|value| !value.is_null())
                                    .map(integer_value)
                                    .unwrap_or(1)
                                    != 0
                        })
                    })
                    .map(|channel| number_value(&channel["rate"]).max(0.0))
                    .filter(|rate| rate.is_finite())
                    .unwrap_or_else(|| {
                        if shop.fee_rate > 0.0 {
                            shop.fee_rate
                        } else {
                            3.0
                        }
                    });
            }
            shop.fee_checked_at = Some(now.to_rfc3339());
        } else if shop.fee_payer != 1 {
            shop.fee_rate = 0.0;
            shop.fee_checked_at = shop.profile_checked_at.clone();
        }

        let mut products = HashMap::new();
        for goods_type in shop.goods_types.clone() {
            for page in 1..=20 {
                let data = self
                    .post_json(
                        "/shopApi/Shop/goodsList",
                        json!({
                            "token": shop.token,
                            "goods_type": goods_type,
                            "category_id": 0,
                            "keywords": "",
                            "current": page,
                            "pageSize": PAGE_SIZE,
                        }),
                        protection,
                        false,
                    )
                    .await?;
                let list = data["list"].as_array().cloned().unwrap_or_default();
                for item in &list {
                    if let Some(product) = normalize_product(item, &shop) {
                        products.insert(product.id.clone(), product);
                    }
                }
                if list.len() < PAGE_SIZE as usize
                    || (page * PAGE_SIZE) >= integer_value(&data["total"])
                {
                    break;
                }
            }
        }
        shop.ok = true;
        shop.error = None;
        shop.failure_count = 0;
        shop.blocked_until = None;
        shop.last_checked_at = Some(now.to_rfc3339());
        shop.last_success_at = shop.last_checked_at.clone();
        Ok((shop, products.into_values().collect()))
    }

    async fn post_json(
        &self,
        path: &str,
        body: Value,
        protection: &Arc<tokio::sync::Mutex<MarketProtection>>,
        optional: bool,
    ) -> Result<Value, String> {
        {
            let guard = protection.lock().await;
            if guard
                .circuit_open_until
                .as_deref()
                .and_then(parse_time)
                .is_some_and(|until| until > Utc::now())
            {
                return Err(guard
                    .circuit_reason
                    .clone()
                    .unwrap_or_else(|| "接口保护冷却中".to_string()));
            }
        }
        let mut attempts = Vec::new();
        let active = {
            let guard = protection.lock().await;
            if API_BASES.contains(&guard.active_api_base.as_str()) {
                guard.active_api_base.clone()
            } else {
                API_BASES[0].to_string()
            }
        };
        let mut bases = vec![active.clone()];
        bases.extend(
            API_BASES
                .iter()
                .filter(|base| **base != active)
                .map(|base| (*base).to_string()),
        );

        for base in bases {
            {
                let guard = protection.lock().await;
                if let Some(last) = guard.last_request_at.as_deref().and_then(parse_time) {
                    let elapsed = (Utc::now() - last).num_milliseconds();
                    if elapsed < REQUEST_SPACING_MS {
                        let wait_ms = (REQUEST_SPACING_MS - elapsed) as u64;
                        drop(guard);
                        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                    }
                }
            }
            let request_at = Utc::now().to_rfc3339();
            {
                let mut guard = protection.lock().await;
                guard.last_request_at = Some(request_at.clone());
                guard.request_timestamps.push(request_at);
            }
            self.checkpoint_protection(protection, false).await;
            let mut headers = HeaderMap::new();
            headers.insert(
                ACCEPT,
                HeaderValue::from_static("application/json, text/plain, */*"),
            );
            headers.insert(
                ACCEPT_LANGUAGE,
                HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.6"),
            );
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124.0 Safari/537.36"));
            if let Ok(value) = HeaderValue::from_str(&base) {
                headers.insert(ORIGIN, value);
            }
            if let Ok(value) = HeaderValue::from_str(&format!(
                "{base}/shop/{}",
                body["token"].as_str().unwrap_or_default()
            )) {
                headers.insert(REFERER, value);
            }
            headers.insert(
                "visitorid",
                HeaderValue::from_str(&uuid::Uuid::new_v4().simple().to_string()).unwrap(),
            );

            let response = match self
                .client
                .post(format!("{base}{path}"))
                .headers(headers)
                .json(&body)
                .timeout(Duration::from_secs(15))
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    attempts.push(error.to_string());
                    continue;
                }
            };
            let status = response.status();
            if status.as_u16() == 429 {
                attempts.push("HTTP 429".to_string());
                continue;
            }
            if status.as_u16() == 403 {
                attempts.push("HTTP 403".to_string());
                continue;
            }
            if !status.is_success() {
                attempts.push(format!("HTTP {}", status.as_u16()));
                if status.is_server_error() {
                    continue;
                }
                return Err(format!("商品接口返回 HTTP {}", status.as_u16()));
            }
            let text = response.text().await.map_err(|error| error.to_string())?;
            if text.trim_start().starts_with('<') {
                attempts.push("安全校验页面".to_string());
                continue;
            }
            let payload: Value = match serde_json::from_str(&text) {
                Ok(payload) => payload,
                Err(_) => {
                    attempts.push("无效 JSON".to_string());
                    continue;
                }
            };
            if integer_value(&payload["code"]) != 1 {
                return Err(
                    string_value(&payload["msg"]).unwrap_or_else(|| "商品接口返回错误".to_string())
                );
            }
            {
                let mut guard = protection.lock().await;
                guard.active_api_base = base.clone();
                guard.fallback_used = base != API_BASES[0];
            }
            return Ok(payload["data"].clone());
        }

        let joined = attempts.join(" / ");
        let has_usable_snapshot = !self.snapshot.read().await.products.is_empty();
        let (protected_duration, reason) = if joined.contains("429") {
            (ChronoDuration::minutes(15), "主备商品接口均返回 429")
        } else if joined.contains("403") {
            (ChronoDuration::hours(1), "主备商品接口均返回 403")
        } else if joined.contains("安全校验") {
            (ChronoDuration::minutes(2), "主备商品接口触发安全校验")
        } else {
            (ChronoDuration::minutes(2), "主备商品接口暂时不可用")
        };
        if !optional {
            let duration = if has_usable_snapshot {
                protected_duration
            } else {
                ChronoDuration::seconds(BOOTSTRAP_RECOVERY_SECONDS)
            };
            let mut guard = protection.lock().await;
            guard.circuit_open_until = Some((Utc::now() + duration).to_rfc3339());
            guard.circuit_reason = Some(reason.to_string());
            guard.mode = "circuit-open".to_string();
            drop(guard);
            self.checkpoint_protection(protection, true).await;
            Err(format!("{reason}，已保留上次成功数据"))
        } else {
            Err(reason.to_string())
        }
    }

    async fn deliver_notification(&self, event: &MarketEvent) {
        let settings = self.database.alert_settings().unwrap_or_default();
        if !should_notify(event, &settings) || is_quiet_time(&settings) {
            return;
        }
        let now = Utc::now().to_rfc3339();
        match crate::market_notification::show_market_notification(&self.app, event) {
            Ok(()) => {
                if let Err(error) = self.database.mark_notified(&event.event_id, "native", &now) {
                    tracing::warn!(%error, event_id = %event.event_id, "记录市场通知投递状态失败");
                }
            }
            Err(error) => {
                tracing::warn!(%error, event_id = %event.event_id, "发送市场系统通知失败");
            }
        }
    }

    async fn checkpoint_protection(&self, protection: &Arc<tokio::sync::Mutex<MarketProtection>>, force: bool) {
        let mut checkpoint = self.protection_checkpoint.lock().await;
        if !force && checkpoint.is_some_and(|previous| previous.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        let snapshot = protection.lock().await.clone();
        match self.database.checkpoint_protection(&snapshot) {
            Ok(()) => *checkpoint = Some(Instant::now()),
            Err(error) => tracing::warn!(%error, "保存市场请求保护检查点失败"),
        }
    }

    pub fn database(&self) -> &MarketDatabase {
        &self.database
    }

    pub async fn reload_snapshot(&self) -> Result<MarketSnapshot, String> {
        let snapshot = self.database.load_snapshot()?;
        *self.snapshot.write().await = snapshot.clone();
        let _ = self.app.emit("market:snapshot", &snapshot);
        Ok(snapshot)
    }
}

fn normalize_product(item: &Value, shop: &MarketShop) -> Option<MarketProduct> {
    let stock_count = integer_value(&item["extend"]["stock_count"]);
    let goods_key = string_value(&item["goods_key"])?;
    let name = string_value(&item["name"])?;
    if stock_count <= 0 || goods_key.is_empty() || name.is_empty() {
        return None;
    }
    let price = number_value(&item["price"]);
    let fee = if shop.fee_payer == 1 {
        money(price * shop.fee_rate.max(0.0) / 100.0)
    } else {
        0.0
    };
    let tags: Vec<String> = item["tags"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|tag| string_value(&tag["name"]).or_else(|| string_value(tag)))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let category = classify_product(&name).map(str::to_string);
    let now = Utc::now().to_rfc3339();
    Some(MarketProduct {
        id: format!("{}:{}", shop.token, goods_key),
        goods_key: goods_key.clone(),
        shop_token: shop.token.clone(),
        shop_name: shop.name.clone(),
        shop_url: format!("{SHOP_BASE}/shop/{}", shop.token),
        name: name.split_whitespace().collect::<Vec<_>>().join(" "),
        url: string_value(&item["link"]).unwrap_or_else(|| format!("{SHOP_BASE}/item/{goods_key}")),
        price,
        fee,
        fee_rate: shop.fee_rate,
        fee_payer: shop.fee_payer,
        total_price: money(price + fee),
        market_price: number_value(&item["market_price"]),
        stock_count,
        source_category: string_value(&item["category"]["name"]).unwrap_or_default(),
        category,
        match_terms: match_terms(&name),
        verification_status: verification_status(
            &name,
            string_value(&item["description"])
                .as_deref()
                .unwrap_or_default(),
            &tags,
        ),
        missing_count: 0,
        first_seen_at: now.clone(),
        last_seen_at: now,
    })
}

fn add_market_signal_events(
    previous: &[MarketProduct],
    current: &[MarketProduct],
    output: &mut Vec<MarketEvent>,
) {
    let previous_stock = previous
        .iter()
        .filter(|item| item.category.is_some())
        .map(|item| item.stock_count)
        .sum::<i64>();
    let current_stock = current
        .iter()
        .filter(|item| item.category.is_some())
        .map(|item| item.stock_count)
        .sum::<i64>();
    let increase = current_stock - previous_stock;
    let rate = if previous_stock > 0 {
        increase as f64 / previous_stock as f64
    } else {
        0.0
    };
    if !previous.is_empty() && increase >= 20.max((previous_stock as f64 * 0.35).ceil() as i64) {
        output.push(MarketEvent {
            seq: 0,
            event_id: format!("stock-surge:{}:{}", Utc::now().timestamp() / 180, current_stock),
            kind: "market.stock_surge".to_string(),
            entity_type: "market".to_string(),
            entity_id: "market".to_string(),
            occurred_at: Utc::now().to_rfc3339(),
            expires_at: Some((Utc::now() + ChronoDuration::hours(6)).to_rfc3339()),
            severity: if increase >= 30 || rate >= 0.5 { "high" } else { "medium" }.to_string(),
            title: "市场集中补库".to_string(),
            body: format!("总库存增加 {increase} 件，当前 {current_stock} 件"),
            section: "analytics".to_string(),
            payload: json!({ "previousStock": previous_stock, "currentStock": current_stock, "increase": increase, "increaseRate": rate }),
            read_at: None,
            notified_at: None,
        });
    }

    let profiles = price_profiles(current);
    for product in current {
        let Some(category) = product.category.as_ref() else {
            continue;
        };
        // Only generate price signals for focus categories
        if category != "k12" && category != "gptplus" && category != "bugteam" {
            continue;
        }
        let Some(profile) = profiles.get(category) else {
            continue;
        };
        if profile.confidence == "low" || profile.weighted_average <= 0.0 {
            continue;
        }
        let direction = if product.total_price > profile.affordable_ceiling {
            Some("high")
        } else if product.total_price <= profile.bargain
            && product.total_price <= profile.weighted_average * 0.95
        {
            Some("low")
        } else {
            None
        };
        let Some(direction) = direction else { continue };
        output.push(MarketEvent {
            seq: 0,
            event_id: format!("price:{direction}:{}:{}", product.id, (product.total_price * 100.0).round() as i64),
            kind: format!("product.price_{direction}"),
            entity_type: "product".to_string(),
            entity_id: product.id.clone(),
            occurred_at: Utc::now().to_rfc3339(),
            expires_at: Some((Utc::now() + ChronoDuration::hours(6)).to_rfc3339()),
            severity: if direction == "low" { "info" } else { "warning" }.to_string(),
            title: if direction == "low" { "发现低价商品" } else { "商品价格偏高" }.to_string(),
            body: format!("{} · {} · ¥{}", product.name, product.shop_name, format_money(product.total_price)),
            section: "products".to_string(),
            payload: json!({ "productId": product.id, "direction": direction, "price": product.total_price, "median": profile.median }),
            read_at: None,
            notified_at: None,
        });
    }

    let previous_profiles = price_profiles(previous);
    let current_profiles = price_profiles(current);
    let previous_by_id = previous
        .iter()
        .map(|product| (product.id.as_str(), product))
        .collect::<HashMap<_, _>>();
    let current_by_id = current
        .iter()
        .map(|product| (product.id.as_str(), product))
        .collect::<HashMap<_, _>>();
    for category in ["k12", "gptplus", "bugteam"] {
        let previous_ceiling = previous_profiles
            .get(category)
            .map(|profile| profile.affordable_ceiling)
            .unwrap_or(f64::INFINITY);
        let current_ceiling = current_profiles
            .get(category)
            .map(|profile| profile.affordable_ceiling)
            .unwrap_or(f64::INFINITY);
        let previous_stock = previous
            .iter()
            .filter(|product| {
                product.category.as_deref() == Some(category)
                    && product.total_price <= previous_ceiling
            })
            .map(|product| product.stock_count)
            .sum::<i64>();
        let current_stock = current
            .iter()
            .filter(|product| {
                product.category.as_deref() == Some(category)
                    && product.total_price <= current_ceiling
            })
            .map(|product| product.stock_count)
            .sum::<i64>();
        let mut gross_increase = 0_i64;
        let mut gross_decrease = 0_i64;
        let mut zeroed_products = Vec::new();
        let mut increased_stores = HashSet::new();

        for product in previous.iter().filter(|product| {
            product.category.as_deref() == Some(category) && product.total_price <= previous_ceiling
        }) {
            let next = current_by_id.get(product.id.as_str()).copied();
            let next_stock = next.map(|item| item.stock_count).unwrap_or_default();
            let delta = next_stock - product.stock_count;
            if delta > 0 {
                gross_increase += delta;
                increased_stores.insert(
                    next.map(|item| item.shop_name.clone())
                        .unwrap_or_else(|| product.shop_name.clone()),
                );
            } else if delta < 0 {
                gross_decrease += delta.abs();
                if next.is_none() {
                    zeroed_products.push(product.name.clone());
                }
            }
        }
        for product in current.iter().filter(|product| {
            product.category.as_deref() == Some(category)
                && product.total_price <= current_ceiling
                && !previous_by_id.contains_key(product.id.as_str())
        }) {
            gross_increase += product.stock_count;
            increased_stores.insert(product.shop_name.clone());
        }

        let net_change = current_stock - previous_stock;
        let stable_threshold = 3_i64.max((previous_stock as f64 * 0.12).ceil() as i64);
        let redistributed = gross_increase.min(gross_decrease);
        let redistribution_rate = if gross_decrease > 0 {
            redistributed as f64 / gross_decrease as f64
        } else {
            0.0
        };
        if net_change.abs() <= stable_threshold
            && zeroed_products.len() >= 2
            && increased_stores.len() >= 2
            && redistribution_rate >= 0.6
        {
            let label = match category {
                "k12" => "K12",
                "gptplus" => "GPT Plus",
                _ => "BUG TEAM",
            };
            output.push(MarketEvent {
                seq: 0,
                event_id: format!(
                    "hoarding:{category}:{}:{}",
                    Utc::now().timestamp() / 180,
                    redistributed
                ),
                kind: "market.suspected_hoarding".to_string(),
                entity_type: "market".to_string(),
                entity_id: category.to_string(),
                occurred_at: Utc::now().to_rfc3339(),
                expires_at: None,
                severity: if zeroed_products.len() >= 4 || increased_stores.len() >= 3 {
                    "high"
                } else {
                    "medium"
                }
                .to_string(),
                title: format!("{label} 疑似扫货/库存转移"),
                body: format!(
                    "{} 个商品归零，{} 家店铺增库，疑似转移 {} 件",
                    zeroed_products.len(),
                    increased_stores.len(),
                    redistributed
                ),
                section: "analytics".to_string(),
                payload: json!({
                    "category": category,
                    "previousStock": previous_stock,
                    "currentStock": current_stock,
                    "netChange": net_change,
                    "grossIncrease": gross_increase,
                    "grossDecrease": gross_decrease,
                    "redistributedStock": redistributed,
                    "zeroedProducts": zeroed_products.into_iter().take(8).collect::<Vec<_>>(),
                    "increasedStores": increased_stores.into_iter().take(8).collect::<Vec<_>>()
                }),
                read_at: None,
                notified_at: None,
            });
        }
    }
}

fn product_event(
    kind: &str,
    product: &MarketProduct,
    title: &str,
    body: String,
    severity: &str,
) -> MarketEvent {
    MarketEvent {
        seq: 0,
        event_id: format!("{}:{}:{}", kind, product.id, Utc::now().timestamp_millis()),
        kind: kind.to_string(),
        entity_type: "product".to_string(),
        entity_id: product.id.clone(),
        occurred_at: Utc::now().to_rfc3339(),
        expires_at: Some((Utc::now() + ChronoDuration::hours(12)).to_rfc3339()),
        severity: severity.to_string(),
        title: title.to_string(),
        body,
        section: "products".to_string(),
        payload: serde_json::to_value(product).unwrap_or_default(),
        read_at: None,
        notified_at: None,
    }
}

fn store_event(
    kind: &str,
    shop: &MarketShop,
    title: &str,
    body: String,
    severity: &str,
) -> MarketEvent {
    MarketEvent {
        seq: 0,
        event_id: format!("{}:{}:{}", kind, shop.token, Utc::now().timestamp_millis()),
        kind: kind.to_string(),
        entity_type: "store".to_string(),
        entity_id: shop.token.clone(),
        occurred_at: Utc::now().to_rfc3339(),
        expires_at: None,
        severity: severity.to_string(),
        title: title.to_string(),
        body,
        section: "stores".to_string(),
        payload: serde_json::to_value(shop).unwrap_or_default(),
        read_at: None,
        notified_at: None,
    }
}

fn should_notify(event: &MarketEvent, settings: &MarketAlertSettings) -> bool {
    if !settings.enabled || !settings.native_enabled {
        return false;
    }
    match event.kind.as_str() {
        "product.available" => {
            let category = event.payload["category"].as_str().unwrap_or_default();
            match category {
                "bugteam" => settings.bug_team_available,
                // K12 and GPT Plus arrivals always notify when alerts enabled
                "k12" | "gptplus" => true,
                _ => false,
            }
        }
        "product.unavailable" => settings.product_unavailable,
        "store.degraded" | "store.recovered" => settings.store_health,
        "market.stock_surge" | "market.suspected_hoarding" => settings.stock_surge,
        value if value.starts_with("product.price_") => settings.price_outlier,
        _ => false,
    }
}

fn is_quiet_time(settings: &MarketAlertSettings) -> bool {
    if !settings.quiet_hours_enabled {
        return false;
    }
    let now = Local::now().time();
    let start = NaiveTime::parse_from_str(&settings.quiet_hours_start, "%H:%M").ok();
    let end = NaiveTime::parse_from_str(&settings.quiet_hours_end, "%H:%M").ok();
    match (start, end) {
        (Some(start), Some(end)) if start <= end => now >= start && now < end,
        (Some(start), Some(end)) => now >= start || now < end,
        _ => false,
    }
}

fn deferred_result(reason: &str, message: &str, snapshot: &MarketSnapshot) -> MarketRefreshResult {
    MarketRefreshResult {
        performed: false,
        reason: Some(reason.to_string()),
        retry_at: snapshot.next_refresh_at.clone(),
        message: Some(message.to_string()),
        snapshot: snapshot.clone(),
    }
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn number_value(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .unwrap_or_default()
}

fn integer_value(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|value| value as i64))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .unwrap_or_default()
}

fn string_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn money(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn format_money(value: f64) -> String {
    let value = format!("{value:.2}");
    value
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

pub async fn analytics_snapshot(
    state: &MarketState,
    range: &str,
) -> Result<MarketAnalyticsSnapshot, String> {
    let (normalized, duration) = match range {
        "7d" => ("7d", ChronoDuration::days(7)),
        "30d" => ("30d", ChronoDuration::days(30)),
        _ => ("24h", ChronoDuration::hours(24)),
    };
    let cutoff = (Utc::now() - duration).to_rfc3339();
    let mut points = state.database.analytics_points(&cutoff)?;
    let total_samples = points.len();
    if points.len() > 360 {
        let source = points;
        let bucket = source.len() as f64 / 360.0;
        points = (0..360)
            .filter_map(|index| {
                source
                    .get((index as f64 * bucket).floor() as usize)
                    .cloned()
            })
            .collect();
    }
    Ok(MarketAnalyticsSnapshot {
        range: normalized.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        points,
        events: state.database.events(Some(&cutoff), 500)?,
        total_samples,
    })
}

pub async fn upsert_shop(
    state: &MarketState,
    input: MarketShopInput,
) -> Result<MarketSnapshot, String> {
    state.database.upsert_shop(&input)?;
    state.reload_snapshot().await
}
