use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::Response,
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
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use crate::capacity::{CapacityLease, CapacityRegistry};
use crate::db::{Account, Db};
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
    pub access_token: Arc<RwLock<String>>,
    capacity: Arc<CapacityRegistry>,
    cooldowns: Mutex<HashMap<CooldownKey, Instant>>,
    sticky_routes: Mutex<HashMap<u64, StickyRoute>>,
    weighted_schedules: Mutex<HashMap<SchedulerKey, WeightedSchedule>>,
    refresh_locks: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

#[derive(Clone)]
struct StreamObserverContext {
    state: Arc<ProxyState>,
    account_id: String,
    capability: RequestCapability,
    route_key: Option<u64>,
    model_hint: Option<String>,
}

impl StreamObserverContext {
    fn record_failure(&self, scope: CooldownScope, message: &str) {
        let _ = self.state.db.set_error(&self.account_id, message);
        self.state.apply_cooldown(
            &self.account_id,
            &self.capability,
            scope,
            Duration::from_secs(20),
        );
        self.state.unbind_route(self.route_key, &self.account_id);
    }

    fn record_usage(&self, usage: UsageBreakdown) {
        let estimate = estimate_cost(&usage, self.model_hint.as_deref());
        if let Err(error) = self.state.db.record_usage(
            &self.account_id,
            &usage,
            estimate.total_cost,
            estimate.unpriced_tokens,
        ) {
            warn!(account_id = %self.account_id, %error, "记录 Token 用量失败");
        }
    }
}

struct StreamBodyObserver {
    context: StreamObserverContext,
    sse: bool,
    buffer: Vec<u8>,
    capture_tail: Vec<u8>,
    observed_model: Option<String>,
    observed_service_tier: Option<String>,
    failure_recorded: bool,
    usage_recorded: bool,
    terminal_seen: bool,
}

impl StreamBodyObserver {
    fn new(context: StreamObserverContext, sse: bool) -> Self {
        Self {
            context,
            sse,
            buffer: Vec::new(),
            capture_tail: Vec::new(),
            observed_model: None,
            observed_service_tier: None,
            failure_recorded: false,
            usage_recorded: false,
            terminal_seen: false,
        }
    }

    fn observe_chunk(&mut self, chunk: &[u8]) {
        self.capture_tail.extend_from_slice(chunk);
        if self.observed_model.is_none() {
            self.observed_model = extract_string_field_from_fragment(&self.capture_tail, "model");
        }
        if self.observed_service_tier.is_none() {
            self.observed_service_tier =
                extract_string_field_from_fragment(&self.capture_tail, "service_tier");
        }
        if stream_has_terminal_marker(&self.capture_tail) {
            self.terminal_seen = true;
        }
        if !self.usage_recorded {
            if let Some(mut usage) = extract_usage_from_fragment(&self.capture_tail) {
                usage.model = usage.model.or_else(|| self.observed_model.clone());
                usage.service_tier = usage
                    .service_tier
                    .or_else(|| self.observed_service_tier.clone());
                self.usage_recorded = true;
                self.context.record_usage(usage);
            }
        }
        const CAPTURE_TAIL_BYTES: usize = 128 * 1024;
        if self.capture_tail.len() > CAPTURE_TAIL_BYTES {
            let discard = self.capture_tail.len() - CAPTURE_TAIL_BYTES;
            self.capture_tail.drain(..discard);
        }

        self.buffer.extend_from_slice(chunk);
        if self.sse {
            while let Some(event_end) = next_sse_event_end(&self.buffer) {
                let event = self.buffer.drain(..event_end).collect::<Vec<_>>();
                self.observe_event(&event);
            }
        } else {
            self.observe_event(&self.buffer.clone());
        }
        if self.buffer.len() > MAX_STREAM_BOOTSTRAP_BYTES {
            let discard = self.buffer.len() - MAX_STREAM_BOOTSTRAP_BYTES;
            self.buffer.drain(..discard);
        }
    }

    fn observe_event(&mut self, event: &[u8]) {
        if stream_has_terminal_event(event) {
            self.terminal_seen = true;
        }
        if !self.failure_recorded {
            if let Some(error) = stream_payload_error(event) {
                self.failure_recorded = true;
                self.context.record_failure(
                    CooldownScope::Capability,
                    &format!("upstream stream failed after commit: {error}"),
                );
            }
        }
        if self.usage_recorded {
            return;
        }
        let Ok(text) = std::str::from_utf8(event) else {
            return;
        };
        let parsed = if self.sse {
            extract_usage_from_sse(text)
        } else {
            extract_usage_from_json_str(text)
        };
        if let Some(mut usage) = parsed {
            if usage.total_tokens > 0 {
                usage.model = usage.model.or_else(|| self.observed_model.clone());
                usage.service_tier = usage
                    .service_tier
                    .or_else(|| self.observed_service_tier.clone());
                self.usage_recorded = true;
                self.context.record_usage(usage);
            }
        }
    }

    fn record_transport_failure(&mut self, error: &str) {
        if self.failure_recorded {
            return;
        }
        self.failure_recorded = true;
        self.context.record_failure(
            CooldownScope::Account,
            &format!("upstream stream transport failed after commit: {error}"),
        );
    }

    fn record_eof(&mut self) {
        if self.sse
            && self.context.capability.endpoint == EndpointFamily::Responses
            && !self.terminal_seen
            && !self.failure_recorded
        {
            self.failure_recorded = true;
            self.context.record_failure(
                CooldownScope::Capability,
                "upstream stream ended after commit without a terminal event",
            );
        }
    }
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
        accounts: Vec<Account>,
        sticky_account_id: Option<&str>,
        capability: &RequestCapability,
    ) -> Vec<Account> {
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
            let first = tier
                .iter()
                .position(|account| sticky_account_id == Some(account.id.as_str()))
                .unwrap_or_else(|| self.weighted_winner(&tier, capability, priority));
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
    access_token: Arc<RwLock<String>>,
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

    let app = Router::new()
        .route("/{*path}", any(proxy_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

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

async fn proxy_handler(
    State(state): State<Arc<ProxyState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if method == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap();
    }
    if uri.path() == "/health" {
        return json_response(StatusCode::OK, json!({"status": "ok"}));
    }
    let authorized_request = {
        let access_token = state
            .access_token
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        authorized(&headers, access_token.as_str())
    };
    if !authorized_request {
        return json_error(
            StatusCode::UNAUTHORIZED,
            "invalid local access token",
            "authentication_error",
        );
    }

    let accounts = match state.db.get_active_accounts() {
        Ok(accounts) => accounts,
        Err(error) => {
            warn!("读取账号失败: {error}");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load accounts",
                "server_error",
            );
        }
    };
    if accounts.is_empty() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no active OpenAI accounts",
            "server_error",
        );
    }

    let capability = RequestCapability::from_request(&uri, &body);
    let accounts = accounts
        .into_iter()
        .filter(|account| account_supports_request(account, &capability))
        .collect::<Vec<_>>();
    if accounts.is_empty() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no active account supports this endpoint",
            "server_error",
        );
    }

    let route_key = request_route_key(&headers, &body);
    let (accounts, retry_after) = state.ordered_accounts(accounts, route_key, &capability);
    if accounts.is_empty() {
        return cooling_down_response(retry_after);
    }

    let requested_stream = !is_compact_path(uri.path()) && request_wants_stream(&body);
    let model_hint = capability.model.clone();
    let startup_deadline = tokio::time::Instant::now() + REQUEST_STARTUP_BUDGET;
    let mut last_error = "all upstream accounts failed".to_string();
    let mut attempted_accounts = 0;

    'accounts: for account in &accounts {
        if attempted_accounts >= MAX_ACCOUNT_ATTEMPTS {
            break;
        }
        if tokio::time::Instant::now() >= startup_deadline {
            last_error = "upstream startup budget exhausted".to_string();
            break;
        }
        let Some(capacity_lease) = state
            .capacity
            .try_acquire(&account.id, account.concurrency)
        else {
            last_error = "all matching upstream accounts are at capacity".to_string();
            continue;
        };
        attempted_accounts += 1;
        let mut ready = match tokio::time::timeout_at(
            startup_deadline,
            ensure_account_ready(&state, account, false),
        )
        .await
        {
            Ok(Ok(account)) => account,
            Ok(Err(error)) => {
                let _ = state.db.set_error(&account.id, &error);
                state.cool_down_account(&account.id, Duration::from_secs(60));
                state.unbind_route(route_key, &account.id);
                last_error = error;
                continue;
            }
            Err(_) => {
                let error = format!(
                    "{}: upstream startup budget exhausted during OAuth refresh",
                    account.name
                );
                let _ = state.db.set_error(&account.id, &error);
                state.cool_down_account(&account.id, Duration::from_secs(60));
                state.unbind_route(route_key, &account.id);
                last_error = error;
                break 'accounts;
            }
        };

        let mut refreshed_after_unauthorized = false;
        loop {
            let response = match tokio::time::timeout_at(
                startup_deadline,
                send_upstream(&state.client, &ready, &method, &uri, &headers, &body),
            )
            .await
            {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => match error {
                    SendUpstreamError::Request(message) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            &message,
                            "invalid_request_error",
                        );
                    }
                    SendUpstreamError::Account(message) | SendUpstreamError::Transport(message) => {
                        let _ = state.db.set_error(&ready.id, &message);
                        state.cool_down_account(&ready.id, Duration::from_secs(20));
                        state.unbind_route(route_key, &ready.id);
                        last_error = message;
                        break;
                    }
                },
                Err(_) => {
                    let error = format!("{}: upstream startup budget exhausted", ready.name);
                    let _ = state.db.set_error(&ready.id, &error);
                    state.cool_down_account(&ready.id, Duration::from_secs(20));
                    state.unbind_route(route_key, &ready.id);
                    last_error = error;
                    break 'accounts;
                }
            };

            let status = response.status();
            if status == StatusCode::UNAUTHORIZED
                && ready.account_type == "oauth"
                && !ready.refresh_token.is_empty()
                && !refreshed_after_unauthorized
            {
                refreshed_after_unauthorized = true;
                match tokio::time::timeout_at(
                    startup_deadline,
                    ensure_account_ready(&state, &ready, true),
                )
                .await
                {
                    Ok(Ok(account)) => {
                        ready = account;
                        continue;
                    }
                    Ok(Err(error)) => {
                        let _ = state.db.set_error(&ready.id, &error);
                        state.cool_down_account(&ready.id, Duration::from_secs(300));
                        state.unbind_route(route_key, &ready.id);
                        last_error = error;
                        break;
                    }
                    Err(_) => {
                        let error = format!(
                            "{}: upstream startup budget exhausted during OAuth refresh",
                            ready.name
                        );
                        let _ = state.db.set_error(&ready.id, &error);
                        state.cool_down_account(&ready.id, Duration::from_secs(60));
                        state.unbind_route(route_key, &ready.id);
                        last_error = error;
                        break 'accounts;
                    }
                }
            }

            let policy = classify_failure(status, capability.model.is_some());
            if policy.switch_account {
                let cooldown = response_cooldown(status, response.headers());
                let summary_deadline = std::cmp::min(
                    startup_deadline,
                    tokio::time::Instant::now() + UPSTREAM_ERROR_BODY_BUDGET,
                );
                let summary =
                    tokio::time::timeout_at(summary_deadline, upstream_error_summary(response))
                        .await
                        .unwrap_or_else(|_| status.to_string());
                let error = format!("{}: {summary}", ready.name);
                let _ = state.db.set_error(&ready.id, &error);
                if let Some(scope) = policy.cooldown_scope {
                    state.apply_cooldown(&ready.id, &capability, scope, cooldown);
                }
                state.unbind_route(route_key, &ready.id);
                last_error = error;
                break;
            }

            if !status.is_success() {
                return hold_capacity_lease(
                    passthrough_client_response(response),
                    capacity_lease,
                );
            }

            let (response, usage) = match tokio::time::timeout_at(
                startup_deadline,
                to_client_response(
                    response,
                    ready.account_type == "oauth",
                    requested_stream,
                    Some(StreamObserverContext {
                        state: Arc::clone(&state),
                        account_id: ready.id.clone(),
                        capability: capability.clone(),
                        route_key,
                        model_hint: model_hint.clone(),
                    }),
                ),
            )
            .await
            {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => {
                    let scope = error.cooldown_scope();
                    let error = format!("{}: {error}", ready.name);
                    let _ = state.db.set_error(&ready.id, &error);
                    state.apply_cooldown(&ready.id, &capability, scope, Duration::from_secs(20));
                    state.unbind_route(route_key, &ready.id);
                    last_error = error;
                    break;
                }
                Err(_) => {
                    let error = format!("{}: upstream startup budget exhausted", ready.name);
                    let _ = state.db.set_error(&ready.id, &error);
                    state.cool_down_account(&ready.id, Duration::from_secs(20));
                    state.unbind_route(route_key, &ready.id);
                    last_error = error;
                    break 'accounts;
                }
            };

            state.clear_cooldown(&ready.id, &capability);
            state.bind_route(route_key, &ready.id);
            let _ = state.db.mark_used(&ready.id);
            if let Some(usage) = usage {
                let estimate = estimate_cost(&usage, model_hint.as_deref());
                if let Err(error) = state.db.record_usage(
                    &ready.id,
                    &usage,
                    estimate.total_cost,
                    estimate.unpriced_tokens,
                ) {
                    warn!(account_id = %ready.id, %error, "记录 Token 用量失败");
                }
            }
            return hold_capacity_lease(response, capacity_lease);
        }
    }

    if attempted_accounts == 0 {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            &last_error,
            "capacity_exhausted",
        );
    }
    json_error(StatusCode::BAD_GATEWAY, &last_error, "upstream_error")
}

fn hold_capacity_lease(response: Response, lease: CapacityLease) -> Response {
    let (parts, body) = response.into_parts();
    let stream = futures::stream::unfold(
        (body.into_data_stream(), lease),
        |(mut body, lease)| async move {
            body.next()
                .await
                .map(|chunk| (chunk, (body, lease)))
        },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}

fn cooling_down_response(retry_after: Option<u64>) -> Response {
    let mut response = json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "all active accounts are temporarily cooling down",
        "upstream_error",
    );
    if let Some(seconds) = retry_after.filter(|seconds| *seconds > 0) {
        if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
    }
    response
}

fn request_route_key(headers: &HeaderMap, body: &[u8]) -> Option<u64> {
    for header in ["session_id", "conversation_id"] {
        if let Some(value) = headers
            .get(header)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(stable_route_hash(header, value));
        }
    }
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let prompt_cache_key = value
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(stable_route_hash("prompt_cache_key", prompt_cache_key))
}

fn stable_route_hash(kind: &str, value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in kind
        .bytes()
        .chain(std::iter::once(b':'))
        .chain(value.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn account_supports_request(account: &Account, capability: &RequestCapability) -> bool {
    let endpoint_supported = account.account_type != "oauth"
        || matches!(
            capability.endpoint,
            EndpointFamily::Responses | EndpointFamily::Models
        );
    endpoint_supported
        && capability
            .model
            .as_deref()
            .map(|model| model_is_allowed(&account.models, model))
            .unwrap_or(true)
}

fn model_is_allowed(configured_models: &[String], requested_model: &str) -> bool {
    if configured_models.is_empty() {
        return true;
    }
    configured_models
        .iter()
        .any(|pattern| wildcard_model_matches(pattern, requested_model))
}

fn wildcard_model_matches(pattern: &str, model: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let model = model.to_ascii_lowercase();
    let pattern = pattern.as_bytes();
    let model = model.as_bytes();
    let (mut pattern_index, mut model_index) = (0, 0);
    let (mut star_index, mut star_match) = (None, 0);

    while model_index < model.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == model[model_index] {
            pattern_index += 1;
            model_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_match = model_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_match += 1;
            model_index = star_match;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn classify_failure(status: StatusCode, has_model: bool) -> FailurePolicy {
    let capability_scope = if has_model {
        CooldownScope::Capability
    } else {
        CooldownScope::Account
    };
    match status.as_u16() {
        401..=403 => FailurePolicy {
            switch_account: true,
            cooldown_scope: Some(CooldownScope::Account),
        },
        404 if has_model => FailurePolicy {
            switch_account: true,
            cooldown_scope: Some(CooldownScope::Capability),
        },
        408 | 429 => FailurePolicy {
            switch_account: true,
            cooldown_scope: Some(capability_scope),
        },
        _ if status.is_server_error() => FailurePolicy {
            switch_account: true,
            cooldown_scope: Some(capability_scope),
        },
        _ => FailurePolicy {
            switch_account: false,
            cooldown_scope: None,
        },
    }
}

fn parse_retry_after(value: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds.clamp(1, 3600)));
    }
    let retry_at = chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()?
        .with_timezone(&chrono::Utc);
    let milliseconds = retry_at.signed_duration_since(now).num_milliseconds();
    if milliseconds <= 0 {
        return None;
    }
    let seconds = ((milliseconds as u64) + 999) / 1000;
    Some(Duration::from_secs(seconds.clamp(1, 3600)))
}

fn response_cooldown(status: StatusCode, headers: &reqwest::header::HeaderMap) -> Duration {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, chrono::Utc::now()))
            .unwrap_or_else(|| Duration::from_secs(60));
    }
    match status.as_u16() {
        401..=403 => Duration::from_secs(300),
        404 => Duration::from_secs(10 * 60),
        408 => Duration::from_secs(20),
        _ if status.is_server_error() => Duration::from_secs(20),
        _ => Duration::from_secs(30),
    }
}

async fn ensure_account_ready(
    state: &ProxyState,
    account: &Account,
    force_refresh: bool,
) -> Result<Account, String> {
    if account.account_type != "oauth" {
        return Ok(account.clone());
    }
    if !force_refresh && oauth_token_is_usable(account) {
        return Ok(account.clone());
    }

    let observed_access_token = account.access_token.clone();
    let refresh_lock = state.refresh_lock(&account.id);
    let _guard = refresh_lock.lock().await;
    let current = state
        .db
        .get_account(&account.id)
        .map_err(|error| format!("重新读取 OAuth 账号失败: {error}"))?
        .ok_or_else(|| "OAuth 账号已不存在".to_string())?;
    if current.status != "active" {
        return Err("OAuth 账号已停用".to_string());
    }
    if force_refresh
        && !current.access_token.is_empty()
        && current.access_token != observed_access_token
    {
        return Ok(current);
    }
    if !force_refresh && oauth_token_is_usable(&current) {
        return Ok(current);
    }

    if current.refresh_token.is_empty() {
        if current.access_token.is_empty() {
            return Err("OAuth 账号缺少 access_token 和 refresh_token".to_string());
        }
        if force_refresh {
            return Err("OAuth 请求未授权且账号无法自动续期".to_string());
        }
        if !oauth_token_is_usable(&current) {
            return Err("OAuth access_token 已过期且无法自动续期".to_string());
        }
        return Ok(current);
    }
    let refreshed = oauth::refresh_account(&state.client, &current).await?;
    state
        .db
        .update_oauth_tokens(&current.id, &refreshed)
        .map_err(|error| format!("保存 OAuth 刷新结果失败: {error}"))
}

fn oauth_token_is_usable(account: &Account) -> bool {
    !account.access_token.is_empty()
        && account
            .expires_at
            .map(|expires| expires > chrono::Utc::now().timestamp() + 120)
            .unwrap_or(true)
}

async fn send_upstream(
    client: &reqwest::Client,
    account: &Account,
    method: &Method,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    body: &[u8],
) -> Result<reqwest::Response, SendUpstreamError> {
    let oauth_account = account.account_type == "oauth";
    let target = if oauth_account {
        oauth_target_url(uri).map_err(SendUpstreamError::Request)?
    } else {
        api_key_target_url(account, uri).map_err(SendUpstreamError::Account)?
    };
    let normalized_body = if oauth_account && is_responses_path(uri.path()) {
        normalize_oauth_body(body, is_compact_path(uri.path()))
            .map_err(SendUpstreamError::Request)?
    } else if !oauth_account {
        include_chat_stream_usage(body, uri.path())
    } else {
        body.to_vec()
    };
    let prompt_cache_key = serde_json::from_slice::<Value>(&normalized_body)
        .ok()
        .and_then(|value| {
            value
                .get("prompt_cache_key")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });

    let mut request = client.request(method.clone(), target);
    let secret = if oauth_account {
        &account.access_token
    } else {
        &account.api_key
    };
    request = request.bearer_auth(secret);

    for name in [
        "accept-language",
        "conversation_id",
        "session_id",
        "x-codex-beta-features",
        "x-codex-installation-id",
        "x-codex-turn-state",
        "x-codex-turn-metadata",
        "x-codex-window-id",
        "openai-organization",
        "openai-project",
    ] {
        if let Some(value) = inbound_headers.get(name) {
            request = request.header(name, value);
        }
    }

    if oauth_account {
        request = request
            .header("User-Agent", CODEX_USER_AGENT)
            .header("originator", "codex_cli_rs")
            .header("version", CODEX_VERSION)
            .header("OpenAI-Beta", "responses=experimental");
        if !account.chatgpt_account_id.is_empty() {
            request = request.header("chatgpt-account-id", &account.chatgpt_account_id);
        }
        if let Some(session) = prompt_cache_key {
            if inbound_headers.get("session_id").is_none() {
                request = request.header("session_id", &session);
            }
            if inbound_headers.get("conversation_id").is_none() {
                request = request.header("conversation_id", &session);
            }
        }
        request = if is_compact_path(uri.path()) || is_models_path(uri.path()) {
            request.header("Accept", "application/json")
        } else {
            request.header("Accept", "text/event-stream")
        };
    } else {
        if let Some(value) = inbound_headers.get("user-agent") {
            request = request.header("User-Agent", value);
        }
        if let Some(value) = inbound_headers.get("openai-beta") {
            request = request.header("OpenAI-Beta", value);
        }
        if let Some(value) = inbound_headers.get("accept") {
            request = request.header("Accept", value);
        }
    }
    if !normalized_body.is_empty() && *method != Method::GET && *method != Method::HEAD {
        request = request
            .header("Content-Type", "application/json")
            .body(normalized_body);
    }
    request.send().await.map_err(|error| {
        SendUpstreamError::Transport(format!("{} 转发失败: {error}", account.name))
    })
}

fn oauth_target_url(uri: &Uri) -> Result<String, String> {
    let path = uri.path();
    let mut target = if is_models_path(path) {
        CHATGPT_CODEX_MODELS_URL.to_string()
    } else if is_responses_path(path) {
        format!(
            "{CHATGPT_CODEX_RESPONSES_URL}{}",
            response_path_suffix(path)
        )
    } else {
        return Err(format!("OAuth 账号不支持端点 {path}"));
    };
    if let Some(query) = uri.query() {
        target.push('?');
        target.push_str(query);
    } else if is_models_path(path) {
        target.push_str("?client_version=");
        target.push_str(CODEX_VERSION);
    }
    Ok(target)
}

fn api_key_target_url(account: &Account, uri: &Uri) -> Result<String, String> {
    let base = if account.base_url.trim().is_empty() {
        "https://api.openai.com"
    } else {
        account.base_url.trim()
    };
    let canonical_path = if is_models_path(uri.path()) {
        "/v1/models".to_string()
    } else if is_responses_path(uri.path()) {
        format!("/v1/responses{}", response_path_suffix(uri.path()))
    } else {
        uri.path().to_string()
    };

    let mut target =
        if base.trim_end_matches('/').ends_with("/v1") && canonical_path.starts_with("/v1/") {
            format!(
                "{}{}",
                base.trim_end_matches('/'),
                canonical_path.trim_start_matches("/v1")
            )
        } else {
            format!("{}{}", base.trim_end_matches('/'), canonical_path)
        };
    reqwest::Url::parse(&target).map_err(|error| format!("Base URL 无效: {error}"))?;
    if let Some(query) = uri.query() {
        target.push('?');
        target.push_str(query);
    }
    Ok(target)
}

fn normalize_oauth_body(body: &[u8], compact: bool) -> Result<Vec<u8>, String> {
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let mut value: Value =
        serde_json::from_slice(body).map_err(|error| format!("请求体不是有效 JSON: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Responses 请求体必须是 JSON 对象".to_string())?;
    if compact {
        object.remove("store");
        object.remove("stream");
    } else {
        object.insert("store".to_string(), Value::Bool(false));
        object.insert("stream".to_string(), Value::Bool(true));
    }
    for key in [
        "max_output_tokens",
        "max_completion_tokens",
        "temperature",
        "top_p",
        "frequency_penalty",
        "presence_penalty",
        "user",
        "metadata",
        "prompt_cache_retention",
        "safety_identifier",
        "stream_options",
    ] {
        object.remove(key);
    }
    if !compact
        && object
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        object.insert(
            "instructions".to_string(),
            Value::String(
                "You are Codex, a coding agent. Follow the user's instructions precisely."
                    .to_string(),
            ),
        );
    }
    if !compact && object.get("reasoning").is_some() {
        let include = object
            .entry("include".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(items) = include.as_array_mut() {
            let required = "reasoning.encrypted_content";
            if !items.iter().any(|item| item.as_str() == Some(required)) {
                items.push(Value::String(required.to_string()));
            }
        }
    }
    if let Some(input) = object.get_mut("input") {
        if let Some(text) = input.as_str() {
            *input = json!([{"type":"message","role":"user","content":text}]);
        } else if let Some(items) = input.as_array_mut() {
            for item in items {
                if item.get("role").and_then(Value::as_str) == Some("system") {
                    if let Some(item) = item.as_object_mut() {
                        item.insert("role".to_string(), Value::String("developer".to_string()));
                    }
                }
            }
        }
    }
    serde_json::to_vec(&value).map_err(|error| format!("序列化请求体失败: {error}"))
}

fn include_chat_stream_usage(body: &[u8], path: &str) -> Vec<u8> {
    if !is_chat_completions_path(path) {
        return body.to_vec();
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };
    let Some(object) = value.as_object_mut() else {
        return body.to_vec();
    };
    if object.get("stream").and_then(Value::as_bool) != Some(true) {
        return body.to_vec();
    }
    let stream_options = object
        .entry("stream_options".to_string())
        .or_insert_with(|| json!({}));
    let Some(stream_options) = stream_options.as_object_mut() else {
        return body.to_vec();
    };
    stream_options.insert("include_usage".to_string(), Value::Bool(true));
    serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
}

async fn to_client_response(
    response: reqwest::Response,
    oauth_account: bool,
    requested_stream: bool,
    stream_observer: Option<StreamObserverContext>,
) -> Result<(Response, Option<UsageBreakdown>), PrepareResponseError> {
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let headers = filtered_response_headers(response.headers());

    if status.is_success()
        && oauth_account
        && !requested_stream
        && is_sse_response(response.headers())
    {
        let text = response.text().await.map_err(|error| {
            PrepareResponseError::Transport(format!("failed to read upstream response: {error}"))
        })?;
        if let Some(error) = stream_payload_error(text.as_bytes()) {
            return Err(PrepareResponseError::Upstream(format!(
                "upstream stream failed: {error}"
            )));
        }
        let usage = extract_usage_from_sse(&text);
        let completed = completed_response_from_sse(&text).ok_or_else(|| {
            PrepareResponseError::Upstream(
                "upstream stream ended without a terminal response event".to_string(),
            )
        })?;
        return Ok((json_response(status, completed), usage));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if !requested_stream && !content_type.contains("text/event-stream") {
        let bytes = response.bytes().await.map_err(|error| {
            PrepareResponseError::Transport(format!("failed to read upstream response: {error}"))
        })?;
        let usage = std::str::from_utf8(&bytes)
            .ok()
            .and_then(extract_usage_from_json_str);
        let mut builder = Response::builder().status(status);
        *builder.headers_mut().unwrap() = headers;
        let resp = builder.body(Body::from(bytes)).unwrap();
        return Ok((resp, usage));
    }

    if status.is_success() && (requested_stream || content_type.contains("text/event-stream")) {
        let sse = content_type.contains("text/event-stream");
        let mut stream = Box::pin(response.bytes_stream());
        let first = read_stream_bootstrap(stream.as_mut(), sse).await?;
        let mut observer = stream_observer.map(|context| StreamBodyObserver::new(context, sse));
        if let Some(observer) = observer.as_mut() {
            observer.observe_chunk(&first);
        }
        let remaining = futures::stream::unfold(
            (stream, observer),
            |(mut stream, mut observer)| async move {
                match stream.next().await {
                    Some(Ok(chunk)) => {
                        if let Some(observer) = observer.as_mut() {
                            observer.observe_chunk(&chunk);
                        }
                        Some((Ok(chunk), (stream, observer)))
                    }
                    Some(Err(error)) => {
                        if let Some(observer) = observer.as_mut() {
                            observer.record_transport_failure(&error.to_string());
                        }
                        Some((Err(std::io::Error::other(error)), (stream, observer)))
                    }
                    None => {
                        if let Some(observer) = observer.as_mut() {
                            observer.record_eof();
                        }
                        None
                    }
                }
            },
        );
        let first = futures::stream::once(async move { Ok::<Bytes, std::io::Error>(first) });
        let stream = first.chain(remaining);
        let mut builder = Response::builder().status(status);
        *builder.headers_mut().unwrap() = headers;
        return Ok((builder.body(Body::from_stream(stream)).unwrap(), None));
    }

    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let mut builder = Response::builder().status(status);
    *builder.headers_mut().unwrap() = headers;
    Ok((builder.body(Body::from_stream(stream)).unwrap(), None))
}

async fn read_stream_bootstrap<S, E>(
    mut stream: Pin<&mut S>,
    sse: bool,
) -> Result<Bytes, PrepareResponseError>
where
    S: Stream<Item = Result<Bytes, E>> + ?Sized,
    E: Display,
{
    let mut buffered = Vec::new();
    loop {
        match stream.as_mut().next().await {
            Some(Ok(chunk)) if chunk.is_empty() => continue,
            Some(Ok(chunk)) => {
                if !sse {
                    if let Some(error) = stream_payload_error(&chunk) {
                        return Err(PrepareResponseError::Upstream(format!(
                            "upstream stream failed before first payload: {error}"
                        )));
                    }
                    return Ok(chunk);
                }
                buffered.extend_from_slice(&chunk);
                if let Some(error) = stream_payload_error(&buffered) {
                    return Err(PrepareResponseError::Upstream(format!(
                        "upstream stream failed before first payload: {error}"
                    )));
                }
                if sse_has_payload(&buffered) {
                    return Ok(Bytes::from(buffered));
                }
                if buffered.len() > MAX_STREAM_BOOTSTRAP_BYTES {
                    return Err(PrepareResponseError::Upstream(
                        "upstream SSE bootstrap exceeded 64 KiB".to_string(),
                    ));
                }
            }
            Some(Err(error)) => {
                return Err(PrepareResponseError::Transport(format!(
                    "upstream stream failed before first payload: {error}"
                )));
            }
            None => {
                return Err(PrepareResponseError::Upstream(
                    "upstream stream ended before first payload".to_string(),
                ))
            }
        }
    }
}

fn sse_has_payload(buffer: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(buffer) else {
        return false;
    };
    let normalized = text.replace("\r\n", "\n");
    let Some(last_delimiter) = normalized.rfind("\n\n") else {
        return false;
    };
    let inspected = &normalized[..last_delimiter];
    inspected.lines().any(|line| {
        line.strip_prefix("data:")
            .map(str::trim)
            .is_some_and(|data| !data.is_empty() && data != "[DONE]")
    })
}

fn next_sse_event_end(buffer: &[u8]) -> Option<usize> {
    for index in 0..buffer.len().saturating_sub(1) {
        if buffer[index..].starts_with(b"\n\n") {
            return Some(index + 2);
        }
        if buffer[index..].starts_with(b"\r\n\r\n") {
            return Some(index + 4);
        }
    }
    None
}

fn stream_has_terminal_event(chunk: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(chunk) else {
        return false;
    };
    if serde_json::from_str::<Value>(text.trim())
        .ok()
        .is_some_and(|value| is_terminal_stream_value(&value))
    {
        return true;
    }
    text.lines().any(|line| {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            return false;
        };
        data == "[DONE]"
            || serde_json::from_str::<Value>(data)
                .ok()
                .is_some_and(|value| is_terminal_stream_value(&value))
    })
}

fn is_terminal_stream_value(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some(
            "response.completed"
                | "response.done"
                | "response.incomplete"
                | "response.failed"
                | "response.cancelled"
                | "response.canceled"
        )
    )
}

fn stream_payload_error(chunk: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(chunk).ok()?;
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        if let Some(error) = stream_error_from_value(&value) {
            return Some(error);
        }
    }
    for line in text.lines() {
        if line
            .strip_prefix("event:")
            .map(str::trim)
            .is_some_and(|event| matches!(event, "error" | "response.failed"))
        {
            return Some("upstream emitted an error event".to_string());
        }
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(error) = stream_error_from_value(&value) {
            return Some(error);
        }
    }
    None
}

fn stream_error_from_value(value: &Value) -> Option<String> {
    let event_type = value.get("type").and_then(Value::as_str);
    let error = value
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            value
                .pointer("/response/error")
                .filter(|error| !error.is_null())
        });
    if !matches!(event_type, Some("error" | "response.failed")) && error.is_none() {
        return None;
    }
    let message = error
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
        })
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or(event_type)
        .unwrap_or("upstream stream error");
    Some(message.chars().take(300).collect())
}

fn completed_response_from_sse(text: &str) -> Option<Value> {
    let mut completed = None;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some(
                "response.completed"
                    | "response.done"
                    | "response.incomplete"
                    | "response.cancelled"
                    | "response.canceled"
            )
        ) {
            completed = value.get("response").cloned().or(Some(value));
        }
    }
    completed
}

fn filtered_response_headers(headers: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut output = HeaderMap::new();
    for (name, value) in headers {
        if matches!(
            name.as_str(),
            "connection" | "transfer-encoding" | "content-length" | "keep-alive"
        ) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            axum::http::HeaderName::from_bytes(name.as_str().as_bytes()),
            axum::http::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            output.append(name, value);
        }
    }
    output
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let api_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok());
    bearer == Some(expected) || api_key == Some(expected)
}

async fn upstream_error_summary(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let summary = body.chars().take(300).collect::<String>();
    if summary.is_empty() {
        status.to_string()
    } else {
        format!("{status} {summary}")
    }
}

fn passthrough_client_response(response: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let headers = filtered_response_headers(response.headers());
    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let mut builder = Response::builder().status(status);
    *builder.headers_mut().unwrap() = headers;
    builder.body(Body::from_stream(stream)).unwrap()
}

fn request_wants_stream(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn extract_model_hint(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("model").and_then(Value::as_str).map(String::from))
}

fn extract_usage_from_json_str(text: &str) -> Option<UsageBreakdown> {
    let value: Value = serde_json::from_str(text).ok()?;
    extract_usage_from_value(&value)
}

fn extract_usage_from_sse(text: &str) -> Option<UsageBreakdown> {
    let normalized = text.replace("\r\n", "\n");
    let mut best = None;
    for event in normalized.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if let Some(usage) = extract_usage_from_value(&value) {
            best = Some(usage);
        }
    }
    best
}

fn extract_usage_from_value(value: &Value) -> Option<UsageBreakdown> {
    let response = value.get("response").unwrap_or(value);
    let usage = response
        .get("usage")
        .or_else(|| value.get("usage"))
        .or_else(|| response.get("usageMetadata"))
        .or_else(|| value.get("usageMetadata"))
        .or_else(|| value.pointer("/data/usage"))?;
    let model = response
        .get("model")
        .or_else(|| value.get("model"))
        .or_else(|| response.get("modelVersion"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let service_tier = response
        .get("service_tier")
        .or_else(|| value.get("service_tier"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let native_cache_buckets = response.get("type").and_then(Value::as_str) == Some("message")
        && (usage.get("cache_read_input_tokens").is_some()
            || usage.get("cache_creation_input_tokens").is_some());
    usage_breakdown(usage, model, service_tier, native_cache_buckets)
}

fn extract_string_field_from_fragment(data: &[u8], field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let key = key.as_bytes();
    for offset in (0..=data.len().saturating_sub(key.len())).rev() {
        if data.get(offset..offset + key.len()) != Some(key) {
            continue;
        }
        let remainder = &data[offset + key.len()..];
        let Some(colon) = remainder.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let remainder = &remainder[colon + 1..];
        let Some(start) = remainder.iter().position(|byte| !byte.is_ascii_whitespace()) else {
            continue;
        };
        if remainder[start] != b'\"' {
            continue;
        }
        let mut escaped = false;
        for end in start + 1..remainder.len() {
            match remainder[end] {
                b'\\' if !escaped => escaped = true,
                b'\"' if !escaped => {
                    return serde_json::from_slice::<String>(&remainder[start..=end]).ok();
                }
                _ => escaped = false,
            }
        }
    }
    None
}

fn extract_usage_from_fragment(data: &[u8]) -> Option<UsageBreakdown> {
    let key = b"\"usage\"";
    let mut offsets = data
        .windows(key.len())
        .enumerate()
        .filter_map(|(offset, candidate)| (candidate == key).then_some(offset))
        .collect::<Vec<_>>();
    offsets.reverse();
    for offset in offsets {
        let remainder = &data[offset + key.len()..];
        let Some(colon) = remainder.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let remainder = &remainder[colon + 1..];
        let Some(start) = remainder.iter().position(|byte| !byte.is_ascii_whitespace()) else {
            continue;
        };
        if remainder[start] != b'{' {
            continue;
        }
        let object = &remainder[start..];
        let Some(end) = balanced_json_object_end(object) else {
            continue;
        };
        let Ok(usage) = serde_json::from_slice::<Value>(&object[..end]) else {
            continue;
        };
        if let Some(usage) = usage_breakdown(&usage, None, None, false) {
            return Some(usage);
        }
    }
    None
}

fn stream_has_terminal_marker(data: &[u8]) -> bool {
    [
        b"response.completed".as_slice(),
        b"response.done".as_slice(),
        b"response.incomplete".as_slice(),
        b"response.failed".as_slice(),
        b"response.cancelled".as_slice(),
        b"response.canceled".as_slice(),
        b"[DONE]".as_slice(),
    ]
    .iter()
    .any(|marker| data.windows(marker.len()).any(|value| value == *marker))
}

fn usage_breakdown(
    usage: &Value,
    model: Option<String>,
    service_tier: Option<String>,
    native_cache_buckets: bool,
) -> Option<UsageBreakdown> {
    let total = token_value(
        usage
            .get("total_tokens")
            .or_else(|| usage.get("totalTokenCount")),
    );
    let mut input = token_value(
        usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .or_else(|| usage.get("promptTokenCount")),
    );
    let mut output = token_value(
        usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .or_else(|| usage.get("candidatesTokenCount")),
    );
    let input_details = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"));
    let cached_tokens = token_value(
        input_details
            .and_then(|details| {
                details
                    .get("cached_tokens")
                    .or_else(|| details.get("cache_read_tokens"))
                    .or_else(|| details.get("cache_read_input_tokens"))
            })
            .or_else(|| usage.get("cache_read_input_tokens"))
            .or_else(|| usage.get("cache_read_tokens"))
            .or_else(|| usage.get("cached_tokens"))
            .or_else(|| usage.get("cachedContentTokenCount")),
    );
    let cache_write_tokens = token_value(
        input_details
            .and_then(|details| {
                details
                    .get("cache_write_tokens")
                    .or_else(|| details.get("cache_creation_tokens"))
                    .or_else(|| details.get("cache_creation_input_tokens"))
            })
            .or_else(|| usage.get("cache_creation_input_tokens"))
            .or_else(|| usage.get("cache_write_input_tokens"))
            .or_else(|| usage.get("cache_write_tokens")),
    );
    let output_details = usage
        .get("output_tokens_details")
        .or_else(|| usage.get("completion_tokens_details"));
    let reasoning_tokens = token_value(
        output_details
            .and_then(|details| details.get("reasoning_tokens"))
            .or_else(|| usage.get("reasoning_tokens"))
            .or_else(|| usage.get("thoughtsTokenCount")),
    );

    if native_cache_buckets {
        input = input
            .saturating_add(cached_tokens)
            .saturating_add(cache_write_tokens);
    }
    if input == 0 && output > 0 && total > output {
        input = total - output;
    }
    if output == 0 && input > 0 && total > input {
        output = total - input;
    }
    let total_tokens = total.max(input.saturating_add(output));
    if total_tokens <= 0 {
        return None;
    }
    Some(
        UsageBreakdown {
            total_tokens,
            input_tokens: input,
            output_tokens: output,
            cached_tokens,
            cache_write_tokens,
            reasoning_tokens,
            model,
            service_tier,
        }
        .normalize(),
    )
}

fn token_value(value: Option<&Value>) -> i64 {
    let Some(value) = value else {
        return 0;
    };
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_f64().map(|value| value.trunc() as i64))
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        .unwrap_or(0)
        .max(0)
}

fn balanced_json_object_end(value: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in value.iter().copied().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'\"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'\"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_chat_completions_path(path: &str) -> bool {
    path.contains("/chat/completions")
}

fn is_sse_response(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false)
}

fn is_models_path(path: &str) -> bool {
    matches!(path, "/models" | "/v1/models" | "/backend-api/codex/models")
}

fn is_responses_path(path: &str) -> bool {
    path == "/responses"
        || path.starts_with("/responses/")
        || path == "/v1/responses"
        || path.starts_with("/v1/responses/")
        || path == "/backend-api/codex/responses"
        || path.starts_with("/backend-api/codex/responses/")
}

fn is_compact_path(path: &str) -> bool {
    response_path_suffix(path).starts_with("/compact")
}

fn response_path_suffix(path: &str) -> &str {
    for prefix in [
        "/backend-api/codex/responses",
        "/v1/responses",
        "/responses",
    ] {
        if let Some(suffix) = path.strip_prefix(prefix) {
            return suffix;
        }
    }
    ""
}

fn json_response(status: StatusCode, value: Value) -> Response {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

fn json_error(status: StatusCode, message: &str, error_type: &str) -> Response {
    json_response(
        status,
        json!({
            "error": {
                "message": message,
                "type": error_type,
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewAccount;

    fn test_state() -> ProxyState {
        ProxyState {
            db: Arc::new(Db::new(std::path::Path::new(":memory:")).unwrap()),
            client: reqwest::Client::new(),
            access_token: Arc::new(RwLock::new(String::new())),
            capacity: Arc::new(CapacityRegistry::default()),
            cooldowns: Mutex::new(HashMap::new()),
            sticky_routes: Mutex::new(HashMap::new()),
            weighted_schedules: Mutex::new(HashMap::new()),
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    fn responses_capability(model: &str) -> RequestCapability {
        RequestCapability {
            endpoint: EndpointFamily::Responses,
            model: Some(model.to_string()),
        }
    }

    fn scheduling_account(id: &str, priority: i64) -> Account {
        Account {
            id: id.to_string(),
            name: id.to_string(),
            account_type: "oauth".to_string(),
            api_key: String::new(),
            access_token: "token".to_string(),
            refresh_token: String::new(),
            refreshable: false,
            id_token: String::new(),
            client_id: String::new(),
            credential_masked: "****".to_string(),
            base_url: String::new(),
            models: Vec::new(),
            weight: 1,
            concurrency: 10,
            chatgpt_account_id: String::new(),
            chatgpt_user_id: String::new(),
            email: String::new(),
            plan_type: String::new(),
            expires_at: None,
            priority,
            status: "active".to_string(),
            last_error: String::new(),
            last_used_at: None,
            request_count: 0,
            created_at: id.to_string(),
            updated_at: id.to_string(),
        }
    }

    #[test]
    fn parses_chat_usage_strings_and_token_details() {
        let usage = extract_usage_from_json_str(
            r#"{"model":"gpt-5.6-sol","service_tier":"priority","usage":{"prompt_tokens":"120","completion_tokens":"30","total_tokens":"150","prompt_tokens_details":{"cached_tokens":"40","cache_creation_tokens":"10"},"completion_tokens_details":{"reasoning_tokens":"12"}}}"#,
        )
        .unwrap();
        assert_eq!(usage.total_tokens, 150);
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 30);
        assert_eq!(usage.cached_tokens, 40);
        assert_eq!(usage.cache_write_tokens, 10);
        assert_eq!(usage.reasoning_tokens, 12);
        assert_eq!(usage.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(usage.service_tier.as_deref(), Some("priority"));
    }

    #[test]
    fn chat_stream_requests_include_usage_without_losing_existing_options() {
        let body = include_chat_stream_usage(
            br#"{"model":"gpt-5","stream":true,"stream_options":{"custom":true}}"#,
            "/v1/chat/completions",
        );
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.pointer("/stream_options/include_usage"), Some(&json!(true)));
        assert_eq!(value.pointer("/stream_options/custom"), Some(&json!(true)));
    }

    #[test]
    fn stream_observer_captures_usage_after_large_terminal_payload() {
        let state = Arc::new(test_state());
        let (account, _) = state
            .db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "sk-large-usage".to_string(),
                ..NewAccount::default()
            })
            .unwrap();
        let capability = responses_capability("gpt-5.6-sol");
        let mut observer = StreamBodyObserver::new(
            StreamObserverContext {
                state: Arc::clone(&state),
                account_id: account.id,
                capability: capability.clone(),
                route_key: None,
                model_hint: capability.model,
            },
            true,
        );
        let filler = "x".repeat(96 * 1024);
        let event = format!(
            "data: {{\"type\":\"response.completed\",\"response\":{{\"model\":\"gpt-5.6-sol\",\"output\":[{{\"encrypted_content\":\"{filler}\"}}],\"usage\":{{\"input_tokens\":120,\"output_tokens\":30,\"input_tokens_details\":{{\"cached_tokens\":40}},\"output_tokens_details\":{{\"reasoning_tokens\":12}}}}}}}}\r\n\r\n"
        );
        for chunk in event.as_bytes().chunks(4093) {
            observer.observe_chunk(chunk);
        }
        observer.record_eof();

        let totals = state.db.usage_totals().unwrap();
        assert_eq!(totals.total_tokens, 150);
        assert_eq!(totals.input_tokens, 120);
        assert_eq!(totals.output_tokens, 30);
        assert_eq!(totals.cached_tokens, 40);
        assert_eq!(totals.reasoning_tokens, 12);
        assert_eq!(totals.unpriced_tokens, 0);
        assert!(totals.total_cost > 0.0);
    }

    #[test]
    fn maps_supported_response_aliases() {
        assert_eq!(response_path_suffix("/v1/responses/compact"), "/compact");
        assert_eq!(
            response_path_suffix("/backend-api/codex/responses/input_tokens"),
            "/input_tokens"
        );
    }

    #[test]
    fn smooth_weighted_round_robin_stays_inside_priority_tiers() {
        let state = test_state();
        let capability = responses_capability("gpt-5");
        let mut primary = scheduling_account("primary", 1);
        primary.weight = 3;
        let secondary = scheduling_account("secondary", 1);
        let backup = scheduling_account("backup", 5);
        let accounts = vec![primary, secondary, backup];

        let selected = (0..4)
            .map(|_| {
                state
                    .order_by_priority(accounts.clone(), None, &capability)
                    .into_iter()
                    .next()
                    .unwrap()
                    .id
            })
            .collect::<Vec<_>>();
        assert_eq!(selected, ["primary", "primary", "secondary", "primary"]);
        assert!(state
            .order_by_priority(accounts, None, &capability)
            .iter()
            .position(|account| account.id == "backup")
            .is_some_and(|position| position >= 2));
    }

    #[test]
    fn weighted_schedules_are_isolated_by_model() {
        let state = test_state();
        let accounts = vec![scheduling_account("a", 1), scheduling_account("b", 1)];
        let gpt5 = responses_capability("gpt-5");
        let mini = responses_capability("gpt-5-mini");

        assert_eq!(
            state.order_by_priority(accounts.clone(), None, &gpt5)[0].id,
            "a"
        );
        assert_eq!(
            state.order_by_priority(accounts.clone(), None, &gpt5)[0].id,
            "b"
        );
        assert_eq!(state.order_by_priority(accounts, None, &mini)[0].id, "a");
    }

    #[test]
    fn oauth_pool_and_relay_share_responses_candidates_with_model_filtering() {
        let oauth = scheduling_account("oauth", 1);
        let mut relay = scheduling_account("relay", 1);
        relay.account_type = "api_key".to_string();
        relay.models = vec!["gpt-5*".to_string(), "o3".to_string()];

        assert!(account_supports_request(
            &oauth,
            &responses_capability("gpt-5")
        ));
        assert!(account_supports_request(
            &relay,
            &responses_capability("gpt-5-mini")
        ));
        assert!(!account_supports_request(
            &relay,
            &responses_capability("gpt-4.1")
        ));
        assert!(model_is_allowed(&relay.models, "O3"));

        let other = RequestCapability {
            endpoint: EndpointFamily::Other,
            model: Some("gpt-5".to_string()),
        };
        assert!(!account_supports_request(&oauth, &other));
        assert!(account_supports_request(&relay, &other));
    }

    #[test]
    fn sticky_session_does_not_bypass_a_higher_priority_tier() {
        let accounts = vec![
            scheduling_account("a", 1),
            scheduling_account("b", 1),
            scheduling_account("bound", 5),
        ];
        let state = test_state();
        let ordered = state
            .order_by_priority(accounts, Some("bound"), &responses_capability("gpt-5"))
            .into_iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        assert_eq!(ordered, ["a", "b", "bound"]);
    }

    #[test]
    fn sticky_session_stays_first_inside_its_priority_tier() {
        let accounts = vec![
            scheduling_account("a", 1),
            scheduling_account("bound", 1),
            scheduling_account("backup", 5),
        ];
        let state = test_state();
        let ordered = state
            .order_by_priority(accounts, Some("bound"), &responses_capability("gpt-5"))
            .into_iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        assert_eq!(ordered, ["bound", "a", "backup"]);
    }

    #[test]
    fn cooldown_falls_back_by_tier_and_reports_when_all_are_cooling() {
        let state = test_state();
        let capability = responses_capability("gpt-5");
        let primary = scheduling_account("primary", 1);
        let backup = scheduling_account("backup", 5);

        state.cool_down_account(&primary.id, Duration::from_secs(60));
        let (ordered, retry_after) =
            state.ordered_accounts(vec![primary.clone(), backup.clone()], None, &capability);
        assert_eq!(ordered[0].id, backup.id);
        assert_eq!(retry_after, None);

        state.cool_down_account(&backup.id, Duration::from_secs(30));
        let (ordered, retry_after) =
            state.ordered_accounts(vec![primary, backup], None, &capability);
        assert!(ordered.is_empty());
        assert!(matches!(retry_after, Some(1..=30)));

        let response = cooling_down_response(retry_after);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().contains_key(header::RETRY_AFTER));
    }

    #[test]
    fn cooldown_is_never_shortened() {
        let state = test_state();
        let key = CooldownKey::Account("account".to_string());
        state.cool_down_account("account", Duration::from_secs(300));
        let first = state.cooldowns.lock().unwrap()[&key];
        state.cool_down_account("account", Duration::from_secs(20));
        let second = state.cooldowns.lock().unwrap()[&key];

        assert!(second >= first);
    }

    #[test]
    fn cooldown_keys_isolate_models_and_honor_account_scope() {
        let state = test_state();
        let model_one = responses_capability("gpt-5");
        let model_two = responses_capability("gpt-5-mini");
        let account_a = scheduling_account("a", 1);
        let account_b = scheduling_account("b", 1);

        state.cool_down_capability(&account_a.id, &model_one, Duration::from_secs(60));
        let (ordered, _) =
            state.ordered_accounts(vec![account_a.clone(), account_b.clone()], None, &model_one);
        assert_eq!(ordered.len(), 1);
        assert_eq!(ordered[0].id, account_b.id);

        let (ordered, _) =
            state.ordered_accounts(vec![account_a.clone(), account_b.clone()], None, &model_two);
        assert_eq!(ordered.len(), 2);
        assert!(ordered.iter().any(|account| account.id == account_a.id));

        state.cool_down_account(&account_a.id, Duration::from_secs(60));
        let (ordered, _) =
            state.ordered_accounts(vec![account_a, account_b.clone()], None, &model_two);
        assert_eq!(ordered.len(), 1);
        assert_eq!(ordered[0].id, account_b.id);
    }

    #[test]
    fn failure_policy_matrix_is_explicit() {
        let cases = [
            (StatusCode::BAD_REQUEST, true, false, None),
            (
                StatusCode::UNAUTHORIZED,
                true,
                true,
                Some(CooldownScope::Account),
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                true,
                true,
                Some(CooldownScope::Account),
            ),
            (
                StatusCode::FORBIDDEN,
                true,
                true,
                Some(CooldownScope::Account),
            ),
            (StatusCode::NOT_FOUND, false, false, None),
            (
                StatusCode::NOT_FOUND,
                true,
                true,
                Some(CooldownScope::Capability),
            ),
            (StatusCode::CONFLICT, true, false, None),
            (StatusCode::UNPROCESSABLE_ENTITY, true, false, None),
            (
                StatusCode::TOO_MANY_REQUESTS,
                true,
                true,
                Some(CooldownScope::Capability),
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                false,
                true,
                Some(CooldownScope::Account),
            ),
            (
                StatusCode::BAD_GATEWAY,
                true,
                true,
                Some(CooldownScope::Capability),
            ),
        ];
        for (status, has_model, switch_account, cooldown_scope) in cases {
            assert_eq!(
                classify_failure(status, has_model),
                FailurePolicy {
                    switch_account,
                    cooldown_scope,
                },
                "status={status}, has_model={has_model}"
            );
        }
    }

    #[test]
    fn retry_after_parses_seconds_and_http_dates() {
        use chrono::TimeZone;

        let now = chrono::Utc
            .with_ymd_and_hms(2026, 7, 31, 12, 0, 0)
            .single()
            .unwrap();
        assert_eq!(
            parse_retry_after("120", now),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after("99999", now),
            Some(Duration::from_secs(3600))
        );
        let future = (now + chrono::Duration::seconds(90)).to_rfc2822();
        assert_eq!(
            parse_retry_after(&future, now),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            parse_retry_after("Fri, 31 Jul 2026 12:01:30 GMT", now),
            Some(Duration::from_secs(90))
        );
        let expired = (now - chrono::Duration::seconds(1)).to_rfc2822();
        assert_eq!(parse_retry_after(&expired, now), None);
        assert_eq!(parse_retry_after("invalid", now), None);
    }

    #[tokio::test]
    async fn stream_bootstrap_rejects_empty_and_error_before_first_payload() {
        let mut empty = Box::pin(futures::stream::empty::<Result<Bytes, &'static str>>());
        assert!(read_stream_bootstrap(empty.as_mut(), true).await.is_err());

        let mut failed = Box::pin(futures::stream::iter(vec![
            Ok::<_, &'static str>(Bytes::from_static(b"data: {\"type\":\"response.fa")),
            Ok(Bytes::from_static(
                b"iled\",\"response\":{\"error\":{\"message\":\"quota\"}}}\n\n",
            )),
        ]));
        let error = read_stream_bootstrap(failed.as_mut(), true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("quota"));
        assert_eq!(error.cooldown_scope(), CooldownScope::Capability);

        let mut disconnected =
            Box::pin(futures::stream::iter(vec![Err::<Bytes, _>("disconnected")]));
        let error = read_stream_bootstrap(disconnected.as_mut(), true)
            .await
            .unwrap_err();
        assert_eq!(error.cooldown_scope(), CooldownScope::Account);

        let mut incomplete = Box::pin(futures::stream::iter(vec![Ok::<_, &'static str>(
            Bytes::from_static(b"data: {\"type\":\"response.created\"}"),
        )]));
        assert!(read_stream_bootstrap(incomplete.as_mut(), true)
            .await
            .is_err());

        let first_payload = Bytes::from_static(b"data: {\"type\":\"response.created\"}\n\n");
        let later_error = Bytes::from_static(b"event: error\n\n");
        let mut committed = Box::pin(futures::stream::iter(vec![
            Ok::<_, &'static str>(Bytes::new()),
            Ok(first_payload.clone()),
            Ok(later_error.clone()),
        ]));
        assert_eq!(
            read_stream_bootstrap(committed.as_mut(), true)
                .await
                .unwrap(),
            first_payload
        );
        assert_eq!(committed.next().await.unwrap().unwrap(), later_error);
    }

    #[test]
    fn stream_observer_records_late_usage_and_failure() {
        let state = Arc::new(test_state());
        let (account, _) = state
            .db
            .upsert_account(&NewAccount {
                account_type: "api_key".to_string(),
                api_key: "sk-stream-observer".to_string(),
                ..NewAccount::default()
            })
            .unwrap();
        let capability = responses_capability("gpt-5");
        let route_key = Some(42);
        state.bind_route(route_key, &account.id);
        let mut observer = StreamBodyObserver::new(
            StreamObserverContext {
                state: Arc::clone(&state),
                account_id: account.id.clone(),
                capability: capability.clone(),
                route_key,
                model_hint: capability.model.clone(),
            },
            true,
        );

        observer.observe_chunk(
            b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3}}}\n\n",
        );
        assert_eq!(state.db.total_tokens().unwrap(), 5);
        assert!(state.sticky_routes.lock().unwrap().contains_key(&42));

        observer.observe_chunk(
            b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"late failure\"}}}\n\n",
        );
        let cooldown_key = capability.cooldown_key(&account.id).unwrap();
        assert!(state.cooldowns.lock().unwrap().contains_key(&cooldown_key));
        assert!(!state.sticky_routes.lock().unwrap().contains_key(&42));
        assert!(state
            .db
            .get_account(&account.id)
            .unwrap()
            .unwrap()
            .last_error
            .contains("late failure"));

        let incomplete_capability = responses_capability("gpt-5-mini");
        state.bind_route(Some(43), &account.id);
        let mut incomplete = StreamBodyObserver::new(
            StreamObserverContext {
                state: Arc::clone(&state),
                account_id: account.id.clone(),
                capability: incomplete_capability.clone(),
                route_key: Some(43),
                model_hint: incomplete_capability.model.clone(),
            },
            true,
        );
        incomplete.observe_chunk(b"data: {\"type\":\"response.created\"}\n\n");
        incomplete.record_eof();
        assert!(state
            .cooldowns
            .lock()
            .unwrap()
            .contains_key(&incomplete_capability.cooldown_key(&account.id).unwrap()));
        assert!(!state.sticky_routes.lock().unwrap().contains_key(&43));
    }

    #[tokio::test]
    async fn refresh_lock_is_account_scoped_and_reuses_peer_result() {
        let state = test_state();
        let same_one = state.refresh_lock("account");
        let same_two = state.refresh_lock("account");
        let other = state.refresh_lock("other");
        assert!(Arc::ptr_eq(&same_one, &same_two));
        assert!(!Arc::ptr_eq(&same_one, &other));

        let (stale, _) = state
            .db
            .upsert_account(&NewAccount {
                account_type: "oauth".to_string(),
                access_token: "old-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at: Some(chrono::Utc::now().timestamp() + 3600),
                ..NewAccount::default()
            })
            .unwrap();
        state
            .db
            .update_oauth_tokens(
                &stale.id,
                &NewAccount {
                    access_token: "new-token".to_string(),
                    refresh_token: "refresh-token".to_string(),
                    expires_at: Some(chrono::Utc::now().timestamp() + 3600),
                    ..NewAccount::default()
                },
            )
            .unwrap();

        let ready = ensure_account_ready(&state, &stale, true).await.unwrap();
        assert_eq!(ready.access_token, "new-token");
    }
}
