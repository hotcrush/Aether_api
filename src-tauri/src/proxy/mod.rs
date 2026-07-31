mod protocol;
mod request;
mod streaming;
mod upstream;

#[cfg(test)]
mod tests;

use protocol::*;
use request::*;
use streaming::*;
use upstream::*;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Extension, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use crate::capacity::{CapacityLease, CapacityRegistry};
use crate::db::{Account, Db, RequestLogStart, RequestLogUsage};
use crate::logger::RequestLogHandle;
use crate::oauth;
use crate::pricing::{estimate_cost, UsageBreakdown};

const CHATGPT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const CHATGPT_CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.144.1 (Windows 11; x86_64) Windows_Terminal";
const CODEX_VERSION: &str = "0.144.1";
const STICKY_ROUTE_TTL: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_STICKY_ROUTES: usize = 4096;
const WEIGHTED_SCHEDULE_TTL: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_WEIGHTED_SCHEDULES: usize = 4096;
const MAX_ACCOUNT_ATTEMPTS: usize = 3;
const REQUEST_STARTUP_BUDGET: Duration = Duration::from_secs(30);
const UPSTREAM_ERROR_BODY_BUDGET: Duration = Duration::from_secs(2);
const MAX_STREAM_BOOTSTRAP_BYTES: usize = 64 * 1024;
const MAX_STREAM_OBSERVER_EVENT_BYTES: usize = 256 * 1024;
const MAX_PROXY_REQUEST_BODY_SIZE: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum EndpointFamily {
    Responses,
    Models,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestCapability {
    endpoint: EndpointFamily,
    model: Option<String>,
}

impl RequestCapability {
    fn from_request(uri: &Uri, body: &[u8]) -> Self {
        let endpoint = if is_responses_path(uri.path()) {
            EndpointFamily::Responses
        } else if is_models_path(uri.path()) {
            EndpointFamily::Models
        } else {
            EndpointFamily::Other
        };
        let model = extract_model_hint(body).and_then(|model| {
            let model = model.trim();
            (!model.is_empty()).then(|| model.to_string())
        });
        Self { endpoint, model }
    }

    fn cooldown_key(&self, account_id: &str) -> Option<CooldownKey> {
        self.model.as_ref().map(|model| CooldownKey::Capability {
            account_id: account_id.to_string(),
            endpoint: self.endpoint,
            model: model.clone(),
        })
    }
}

#[derive(Clone)]
struct ProxyRequestLogContext {
    db: Arc<Db>,
    request_id: String,
    method: String,
    path: String,
    endpoint_family: String,
    model: String,
    streaming: bool,
}

impl ProxyRequestLogContext {
    fn new(
        state: &Arc<ProxyState>,
        method: &Method,
        uri: &Uri,
        capability: &RequestCapability,
        streaming: bool,
    ) -> Self {
        Self {
            db: Arc::clone(&state.db),
            request_id: uuid::Uuid::new_v4().simple().to_string(),
            method: method.as_str().to_string(),
            path: uri.path().to_string(),
            endpoint_family: endpoint_family_name(uri.path()),
            model: capability.model.clone().unwrap_or_default(),
            streaming,
        }
    }

    fn begin_attempt(
        &self,
        account: Option<&Account>,
        attempt_index: i64,
    ) -> Option<RequestLogHandle> {
        let start = RequestLogStart {
            request_id: self.request_id.clone(),
            attempt_index,
            account_id: account.map(|account| account.id.clone()),
            account_name: account
                .map(|account| account.name.clone())
                .unwrap_or_default(),
            account_type: account
                .map(|account| account.account_type.clone())
                .unwrap_or_default(),
            source: "proxy".to_string(),
            method: self.method.clone(),
            path: self.path.clone(),
            endpoint_family: self.endpoint_family.clone(),
            model: self.model.clone(),
            streaming: self.streaming,
        };
        match RequestLogHandle::begin(Arc::clone(&self.db), &start) {
            Ok(handle) => Some(handle),
            Err(error) => {
                warn!(request_id = %self.request_id, %error, "创建请求日志失败");
                None
            }
        }
    }

    fn record_local_failure(&self, http_status: StatusCode, message: &str) {
        if let Some(log) = self.begin_attempt(None, 0) {
            log.mark_response(http_status.as_u16());
            log.finish("error", Some(message));
        }
    }
}

fn endpoint_family_name(path: &str) -> String {
    if is_responses_path(path) {
        "responses"
    } else if is_models_path(path) {
        "models"
    } else if is_chat_completions_path(path) {
        "chat_completions"
    } else if path.trim_end_matches('/').ends_with("/embeddings") {
        "embeddings"
    } else {
        "other"
    }
    .to_string()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum CooldownKey {
    Account(String),
    Capability {
        account_id: String,
        endpoint: EndpointFamily,
        model: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CooldownScope {
    Account,
    Capability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FailurePolicy {
    switch_account: bool,
    cooldown_scope: Option<CooldownScope>,
}

#[derive(Debug)]
enum SendUpstreamError {
    Request(String),
    Account(String),
    Transport(String),
}

impl Display for SendUpstreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(message) | Self::Account(message) | Self::Transport(message) => {
                formatter.write_str(message)
            }
        }
    }
}

#[derive(Debug)]
enum PrepareResponseError {
    Transport(String),
    Upstream(String),
}

impl PrepareResponseError {
    fn cooldown_scope(&self) -> CooldownScope {
        match self {
            Self::Transport(_) => CooldownScope::Account,
            Self::Upstream(_) => CooldownScope::Capability,
        }
    }
}

impl Display for PrepareResponseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(message) | Self::Upstream(message) => formatter.write_str(message),
        }
    }
}

struct StickyRoute {
    account_id: String,
    last_seen: Instant,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SchedulerKey {
    endpoint: EndpointFamily,
    model: Option<String>,
    priority: i64,
}

struct WeightedSchedule {
    current: HashMap<String, i64>,
    last_seen: Instant,
}

pub struct ProxyState {
    pub db: Arc<Db>,
    pub client: reqwest::Client,
    pub access_token: Arc<arc_swap::ArcSwap<String>>,
    capacity: Arc<CapacityRegistry>,
    cooldowns: Mutex<HashMap<CooldownKey, Instant>>,
    sticky_routes: Mutex<HashMap<u64, StickyRoute>>,
    weighted_schedules: Mutex<HashMap<SchedulerKey, WeightedSchedule>>,
    refresh_locks: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

#[derive(Clone, Copy)]
pub(super) struct ProxyRequestBodyLimit(pub(super) usize);

#[derive(Clone)]
pub(super) struct BodyLimitMiddlewareState {
    pub(super) proxy: Arc<ProxyState>,
    pub(super) limit: usize,
}

impl ProxyState {
    fn ordered_accounts(
        &self,
        accounts: Vec<Account>,
        route_key: Option<u64>,
        capability: &RequestCapability,
    ) -> (Vec<Account>, Option<u64>) {
        let now = Instant::now();
        let sticky_id = route_key.and_then(|key| self.sticky_account(key, now));
        let mut cooldowns = self
            .cooldowns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cooldowns.retain(|_, until| *until > now);
        let mut earliest_available = None;
        let available = accounts
            .into_iter()
            .filter_map(|account| {
                let account_key = CooldownKey::Account(account.id.clone());
                let capability_key = capability.cooldown_key(&account.id);
                let blocked_until = cooldowns
                    .get(&account_key)
                    .copied()
                    .into_iter()
                    .chain(
                        capability_key
                            .as_ref()
                            .and_then(|key| cooldowns.get(key).copied()),
                    )
                    .max();
                if let Some(until) = blocked_until {
                    earliest_available = Some(
                        earliest_available.map_or(until, |earliest: Instant| earliest.min(until)),
                    );
                    None
                } else {
                    Some(account)
                }
            })
            .collect::<Vec<_>>();
        drop(cooldowns);

        let retry_after = if available.is_empty() {
            earliest_available.map(|until| {
                let remaining = until.saturating_duration_since(now);
                remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
            })
        } else {
            None
        };
        (
            self.order_by_priority(available, sticky_id.as_deref(), capability),
            retry_after,
        )
    }

    fn order_by_priority(
        &self,
        mut accounts: Vec<Account>,
        sticky_account_id: Option<&str>,
        capability: &RequestCapability,
    ) -> Vec<Account> {
        if let Some(sticky_index) = accounts
            .iter()
            .position(|account| sticky_account_id == Some(account.id.as_str()))
        {
            let sticky = accounts.remove(sticky_index);
            accounts.sort_by(|left, right| {
                left.priority
                    .cmp(&right.priority)
                    .then_with(|| left.created_at.cmp(&right.created_at))
                    .then_with(|| left.id.cmp(&right.id))
            });
            accounts.insert(0, sticky);
            return accounts;
        }

        let mut tiers = BTreeMap::<i64, Vec<Account>>::new();
        for account in accounts {
            tiers.entry(account.priority).or_default().push(account);
        }

        let mut ordered = Vec::new();
        for (priority, mut tier) in tiers {
            tier.sort_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.id.cmp(&right.id))
            });
            let first = self.weighted_winner(&tier, capability, priority);
            if first > 0 {
                let selected = tier.remove(first);
                tier.insert(0, selected);
            }
            ordered.extend(tier);
        }
        ordered
    }

    fn weighted_winner(
        &self,
        accounts: &[Account],
        capability: &RequestCapability,
        priority: i64,
    ) -> usize {
        if accounts.len() <= 1 {
            return 0;
        }

        let now = Instant::now();
        let key = SchedulerKey {
            endpoint: capability.endpoint,
            model: capability.model.clone(),
            priority,
        };
        let mut schedules = self
            .weighted_schedules
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        schedules
            .retain(|_, schedule| now.duration_since(schedule.last_seen) < WEIGHTED_SCHEDULE_TTL);
        if schedules.len() >= MAX_WEIGHTED_SCHEDULES && !schedules.contains_key(&key) {
            if let Some(oldest) = schedules
                .iter()
                .min_by_key(|(_, schedule)| schedule.last_seen)
                .map(|(key, _)| key.clone())
            {
                schedules.remove(&oldest);
            }
        }

        let schedule = schedules.entry(key).or_insert_with(|| WeightedSchedule {
            current: HashMap::new(),
            last_seen: now,
        });
        schedule.last_seen = now;
        schedule
            .current
            .retain(|id, _| accounts.iter().any(|account| account.id == *id));

        let mut total_weight = 0_i64;
        let mut winner = 0;
        let mut winner_current = i64::MIN;
        for (index, account) in accounts.iter().enumerate() {
            let weight = account.weight.clamp(1, 1000);
            total_weight = total_weight.saturating_add(weight);
            let current = schedule.current.entry(account.id.clone()).or_default();
            *current = current.saturating_add(weight);
            if *current > winner_current {
                winner = index;
                winner_current = *current;
            }
        }
        if let Some(current) = schedule.current.get_mut(&accounts[winner].id) {
            *current = current.saturating_sub(total_weight);
        }
        winner
    }

    fn sticky_account(&self, key: u64, now: Instant) -> Option<String> {
        let mut routes = self
            .sticky_routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        routes.retain(|_, route| now.duration_since(route.last_seen) < STICKY_ROUTE_TTL);
        let route = routes.get_mut(&key)?;
        route.last_seen = now;
        Some(route.account_id.clone())
    }

    fn bind_route(&self, key: Option<u64>, account_id: &str) {
        let Some(key) = key else { return };
        let now = Instant::now();
        let mut routes = self
            .sticky_routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        routes.retain(|_, route| now.duration_since(route.last_seen) < STICKY_ROUTE_TTL);
        if routes.len() >= MAX_STICKY_ROUTES && !routes.contains_key(&key) {
            if let Some(oldest) = routes
                .iter()
                .min_by_key(|(_, route)| route.last_seen)
                .map(|(key, _)| *key)
            {
                routes.remove(&oldest);
            }
        }
        routes.insert(
            key,
            StickyRoute {
                account_id: account_id.to_string(),
                last_seen: now,
            },
        );
    }

    fn unbind_route(&self, key: Option<u64>, account_id: &str) {
        let Some(key) = key else { return };
        let mut routes = self
            .sticky_routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if routes.get(&key).map(|route| route.account_id.as_str()) == Some(account_id) {
            routes.remove(&key);
        }
    }

    fn cool_down_key(&self, key: CooldownKey, duration: Duration) {
        let until = Instant::now() + duration;
        let mut cooldowns = self
            .cooldowns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cooldowns
            .entry(key)
            .and_modify(|current| *current = (*current).max(until))
            .or_insert(until);
    }

    fn cool_down_account(&self, account_id: &str, duration: Duration) {
        self.cool_down_key(CooldownKey::Account(account_id.to_string()), duration);
    }

    fn cool_down_capability(
        &self,
        account_id: &str,
        capability: &RequestCapability,
        duration: Duration,
    ) {
        let key = capability
            .cooldown_key(account_id)
            .unwrap_or_else(|| CooldownKey::Account(account_id.to_string()));
        self.cool_down_key(key, duration);
    }

    fn apply_cooldown(
        &self,
        account_id: &str,
        capability: &RequestCapability,
        scope: CooldownScope,
        duration: Duration,
    ) {
        match scope {
            CooldownScope::Account => self.cool_down_account(account_id, duration),
            CooldownScope::Capability => {
                self.cool_down_capability(account_id, capability, duration)
            }
        }
    }

    fn clear_cooldown(&self, account_id: &str, capability: &RequestCapability) {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cooldowns.remove(&CooldownKey::Account(account_id.to_string()));
        if let Some(key) = capability.cooldown_key(account_id) {
            cooldowns.remove(&key);
        }
    }

    fn refresh_lock(&self, account_id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .refresh_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(account_id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(account_id.to_string(), Arc::downgrade(&lock));
        lock
    }
}

pub async fn start_proxy_server(
    db: Arc<Db>,
    port: u16,
    access_token: Arc<arc_swap::ArcSwap<String>>,
    running: Arc<AtomicBool>,
    capacity: Arc<CapacityRegistry>,
) {
    let state = Arc::new(ProxyState {
        db,
        client: crate::dns::build_client(600, 15),
        access_token,
        capacity,
        cooldowns: Mutex::new(HashMap::new()),
        sticky_routes: Mutex::new(HashMap::new()),
        weighted_schedules: Mutex::new(HashMap::new()),
        refresh_locks: Mutex::new(HashMap::new()),
    });

    let app = build_proxy_router(state, MAX_PROXY_REQUEST_BODY_SIZE);

    let address = format!("127.0.0.1:{port}");
    let listener = match tokio::net::TcpListener::bind(&address).await {
        Ok(listener) => listener,
        Err(error) => {
            warn!("本地代理端口绑定失败 {address}: {error}");
            return;
        }
    };
    running.store(true, Ordering::Release);
    info!("本地反向代理已启动: http://{address}");
    if let Err(error) = axum::serve(listener, app).await {
        warn!("本地代理服务已停止: {error}");
    }
    running.store(false, Ordering::Release);
}

fn build_proxy_router(state: Arc<ProxyState>, body_limit: usize) -> Router {
    let limit_state = BodyLimitMiddlewareState {
        proxy: Arc::clone(&state),
        limit: body_limit,
    };
    Router::new()
        .route("/{*path}", any(proxy_handler))
        .layer(DefaultBodyLimit::max(body_limit))
        .layer(axum::middleware::from_fn_with_state(
            limit_state,
            enforce_content_length_limit,
        ))
        .layer(Extension(ProxyRequestBodyLimit(body_limit)))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}
