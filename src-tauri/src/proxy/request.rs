use super::*;
use tauri::Emitter;

pub(super) async fn enforce_content_length_limit(
    State(limit_state): State<BodyLimitMiddlewareState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let content_length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if content_length.is_some_and(|length| length > limit_state.limit as u64) {
        let capability = RequestCapability::from_request(request.uri(), &[]);
        let log_context = ProxyRequestLogContext::new(
            &limit_state.proxy,
            request.method(),
            request.uri(),
            &capability,
            false,
            "http",
        );
        let message = request_body_limit_message(limit_state.limit);
        log_context.record_local_failure(StatusCode::PAYLOAD_TOO_LARGE, &message);
        return request_body_limit_response(limit_state.limit);
    }
    next.run(request).await
}

pub(super) async fn proxy_handler(
    State(state): State<Arc<ProxyState>>,
    websocket: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Extension(body_limit): Extension<ProxyRequestBodyLimit>,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
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
    if let Ok(websocket) = websocket {
        return websocket_upgrade_response(state, method, uri, headers, websocket).await;
    }

    let body = match body {
        Ok(body) => body,
        Err(rejection) => {
            let rejection_status = rejection.into_response().status();
            let capability = RequestCapability::from_request(&uri, &[]);
            let request_log =
                ProxyRequestLogContext::new(&state, &method, &uri, &capability, false, "http");
            if rejection_status == StatusCode::PAYLOAD_TOO_LARGE {
                let message = request_body_limit_message(body_limit.0);
                request_log.record_local_failure(StatusCode::PAYLOAD_TOO_LARGE, &message);
                return request_body_limit_response(body_limit.0);
            }
            request_log
                .record_local_failure(StatusCode::BAD_REQUEST, "failed to read request body");
            return json_error(
                StatusCode::BAD_REQUEST,
                "failed to read request body",
                "invalid_request_error",
            );
        }
    };

    let capability = RequestCapability::from_request(&uri, &body);
    let requested_stream = !is_compact_path(uri.path()) && request_wants_stream(&body);
    let transport = if requested_stream { "sse" } else { "http" };
    let request_log = ProxyRequestLogContext::new(
        &state,
        &method,
        &uri,
        &capability,
        requested_stream,
        transport,
    );
    let authorized_request = {
        let access_token = state.access_token.load();
        authorized(&headers, access_token.as_str())
    };
    if !authorized_request {
        request_log.record_local_failure(StatusCode::UNAUTHORIZED, "invalid local access token");
        return json_error(
            StatusCode::UNAUTHORIZED,
            "invalid local access token",
            "authentication_error",
        );
    }
    if !is_forwardable_responses_path(uri.path()) {
        request_log.record_local_failure(StatusCode::NOT_FOUND, "unsupported responses subpath");
        return json_error(
            StatusCode::NOT_FOUND,
            "unsupported responses subpath",
            "not_found_error",
        );
    }

    let image_settings = state.image_generation.load_full();
    let dedicated_image = capability.image_generation && image_settings.enabled;
    let accounts = if dedicated_image {
        vec![image_generation::dedicated_account(&image_settings)]
    } else {
        match state.db.get_active_accounts_async().await {
            Ok(accounts) => accounts,
            Err(error) => {
                warn!("读取账号失败: {error}");
                request_log.record_local_failure(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load accounts",
                );
                return json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load accounts",
                    "server_error",
                );
            }
        }
    };
    if accounts.is_empty() {
        request_log
            .record_local_failure(StatusCode::SERVICE_UNAVAILABLE, "no active OpenAI accounts");
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no active OpenAI accounts",
            "server_error",
        );
    }

    let accounts = accounts
        .into_iter()
        .filter(|account| account_supports_request(account, &capability))
        .collect::<Vec<_>>();
    if accounts.is_empty() {
        request_log.record_local_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "no active account supports this endpoint",
        );
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no active account supports this endpoint",
            "server_error",
        );
    }

    if !dedicated_image {
        let cost_guard = state.cost_guard.load();
        if cost_guard.enabled
            && accounts
                .iter()
                .all(|account| !cost_guard.allows(account.rate_multiplier))
        {
            let message = "all matching upstream accounts are excluded by cost protection";
            request_log.record_local_failure(StatusCode::SERVICE_UNAVAILABLE, message);
            return json_error(StatusCode::SERVICE_UNAVAILABLE, message, "cost_protection");
        }
        drop(cost_guard);
    }

    let route_key = request_route_key(&headers, &body);
    if body.len() >= LARGE_REQUEST_WARNING_BYTES {
        warn!(
            request_bytes = body.len(),
            route_key, "Responses 请求体超过 15 MiB，将保留完整上下文并启用大请求传输保护"
        );
    }
    let (accounts, retry_after, stream_fail_open) =
        state.ordered_accounts_for_request(accounts, route_key, &capability);
    if stream_fail_open {
        warn!(
            model = capability.model.as_deref().unwrap_or(""),
            "所有匹配账号均因流式断流暂时隔离，临时放行隔离账号"
        );
    }
    if accounts.is_empty() {
        request_log.record_local_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "all active accounts are temporarily cooling down",
        );
        return cooling_down_response(retry_after);
    }

    let model_hint = capability.model.clone();
    let startup_deadline = tokio::time::Instant::now() + REQUEST_STARTUP_BUDGET;
    let mut last_error = "all upstream accounts failed".to_string();
    let mut attempted_accounts = 0;
    let mut pending_retry: Option<(RequestLogHandle, String)> = None;

    'accounts: for account in &accounts {
        if attempted_accounts >= MAX_ACCOUNT_ATTEMPTS {
            break;
        }
        if tokio::time::Instant::now() >= startup_deadline {
            last_error = "upstream startup budget exhausted".to_string();
            break;
        }
        let Some(capacity_lease) = state.try_acquire_capacity(account) else {
            last_error = "all matching upstream accounts are at capacity".to_string();
            continue;
        };
        if !dedicated_image && !state.cost_guard.load().allows(account.rate_multiplier) {
            last_error =
                "all matching upstream accounts are excluded by cost protection".to_string();
            continue;
        }
        if let Some((log, message)) = pending_retry.take() {
            log.finish("retry", Some(&message));
        }
        attempted_accounts += 1;
        let attempt_started = tokio::time::Instant::now();
        let attempt_log = request_log.begin_attempt(Some(account), attempted_accounts as i64);
        let mut ready = match tokio::time::timeout_at(
            startup_deadline,
            ensure_account_ready(&state, account, false),
        )
        .await
        {
            Ok(Ok(account)) => account,
            Ok(Err(error)) => {
                if let Some(log) = &attempt_log {
                    pending_retry = Some((log.clone(), error.clone()));
                }
                if !image_generation::is_dedicated_account(&account) {
                    let _ = state.db.set_error_async(&account.id, &error).await;
                }
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
                if let Some(log) = &attempt_log {
                    log.finish("error", Some(&error));
                }
                if !image_generation::is_dedicated_account(&account) {
                    let _ = state.db.set_error_async(&account.id, &error).await;
                }
                state.cool_down_account(&account.id, Duration::from_secs(60));
                state.unbind_route(route_key, &account.id);
                last_error = error;
                break 'accounts;
            }
        };

        let mut refreshed_after_unauthorized = false;
        let mut load_shed_retries = 0usize;
        let codex_version = crate::codex_identity::current_version(&state.codex_version);
        loop {
            let client = state.client.load_full();
            let mut upstream_headers = headers.clone();
            state.guard_codex_turn_state(route_key, &ready, &mut upstream_headers);
            let response = match tokio::time::timeout_at(
                startup_deadline,
                send_upstream(
                    &client,
                    &ready,
                    &method,
                    &uri,
                    &upstream_headers,
                    &body,
                    &codex_version,
                    state.codex_fingerprint.load().mode,
                ),
            )
            .await
            {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => match error {
                    SendUpstreamError::Request(message) => {
                        if let Some(log) = &attempt_log {
                            log.mark_response(StatusCode::BAD_REQUEST.as_u16());
                            log.finish("error", Some(&message));
                        }
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            &message,
                            "invalid_request_error",
                        );
                    }
                    SendUpstreamError::Account(message) | SendUpstreamError::Transport(message) => {
                        if let Some(log) = &attempt_log {
                            pending_retry = Some((log.clone(), message.clone()));
                        }
                        if !image_generation::is_dedicated_account(&ready) {
                            let _ = state.db.set_error_async(&ready.id, &message).await;
                        }
                        state.cool_down_account(&ready.id, Duration::from_secs(20));
                        state.unbind_route(route_key, &ready.id);
                        last_error = message;
                        break;
                    }
                },
                Err(_) => {
                    let error = format!("{}: upstream startup budget exhausted", ready.name);
                    if let Some(log) = &attempt_log {
                        log.finish("error", Some(&error));
                    }
                    if !image_generation::is_dedicated_account(&ready) {
                        let _ = state.db.set_error_async(&ready.id, &error).await;
                    }
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
                        if let Some(log) = &attempt_log {
                            log.mark_response(status.as_u16());
                            pending_retry = Some((log.clone(), error.clone()));
                        }
                        if !image_generation::is_dedicated_account(&ready) {
                            let _ = state.db.set_error_async(&ready.id, &error).await;
                        }
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
                        if let Some(log) = &attempt_log {
                            log.mark_response(status.as_u16());
                            log.finish("error", Some(&error));
                        }
                        let _ = state.db.set_error_async(&ready.id, &error).await;
                        state.cool_down_account(&ready.id, Duration::from_secs(60));
                        state.unbind_route(route_key, &ready.id);
                        last_error = error;
                        break 'accounts;
                    }
                }
            }

            persist_codex_quota_headers(
                &state,
                &ready,
                response.headers(),
                status == StatusCode::TOO_MANY_REQUESTS,
            );
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok());
            let mut policy = classify_failure_with_content_type(
                status,
                capability.model.is_some(),
                content_type,
            );
            if policy.switch_account {
                if let Some(log) = &attempt_log {
                    log.mark_response(status.as_u16());
                }
                let cooldown = response_cooldown(status, response.headers());
                let summary_deadline = std::cmp::min(
                    startup_deadline,
                    tokio::time::Instant::now() + UPSTREAM_ERROR_BODY_BUDGET,
                );
                let (summary, html_body) = tokio::time::timeout_at(
                    summary_deadline,
                    upstream_error_summary_and_html(response),
                )
                .await
                .unwrap_or_else(|_| (status.to_string(), false));
                if status == StatusCode::FORBIDDEN && html_body {
                    policy = classify_failure_with_content_type(
                        status,
                        capability.model.is_some(),
                        Some("text/html"),
                    );
                }
                let error = format!("{}: {summary}", ready.name);
                if let Some(log) = &attempt_log {
                    pending_retry = Some((log.clone(), error.clone()));
                }
                if !image_generation::is_dedicated_account(&ready)
                    && !(status == StatusCode::FORBIDDEN && html_body)
                {
                    let _ = state.db.set_error_async(&ready.id, &error).await;
                }
                if let Some(scope) = policy.cooldown_scope {
                    state.apply_cooldown(&ready.id, &capability, scope, cooldown);
                }
                state.unbind_route(route_key, &ready.id);
                last_error = error;
                break;
            }

            if !status.is_success() {
                if let Some(log) = &attempt_log {
                    log.mark_response(status.as_u16());
                    log.finish("error", Some(&status.to_string()));
                }
                return hold_capacity_lease(passthrough_client_response(response), capacity_lease);
            }

            let first_event_deadline = std::cmp::min(
                startup_deadline,
                tokio::time::Instant::now() + UPSTREAM_FIRST_EVENT_TIMEOUT,
            );
            let upstream_turn_state = response.headers().get("x-codex-turn-state").cloned();
            let (response, usage, completion_deferred) = match tokio::time::timeout_at(
                first_event_deadline,
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
                        request_log: attempt_log.clone(),
                    }),
                ),
            )
            .await
            {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => {
                    if let Some(log) = &attempt_log {
                        log.mark_response(status.as_u16());
                    }
                    if ready.account_type == "oauth" && error.is_transient_load_shed() {
                        let message = format!("{}: {error}", ready.name);
                        if load_shed_retries < MAX_SAME_ACCOUNT_LOAD_SHED_RETRIES {
                            load_shed_retries += 1;
                            let delay = Duration::from_millis(250 * load_shed_retries as u64);
                            if tokio::time::Instant::now() + delay < startup_deadline {
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        }
                        if let Some(log) = &attempt_log {
                            pending_retry = Some((log.clone(), message.clone()));
                        }
                        state.unbind_route(route_key, &ready.id);
                        last_error = message;
                        break;
                    }
                    let scope = error.cooldown_scope();
                    let error = format!("{}: {error}", ready.name);
                    if let Some(log) = &attempt_log {
                        pending_retry = Some((log.clone(), error.clone()));
                    }
                    if !image_generation::is_dedicated_account(&ready) {
                        let _ = state.db.set_error_async(&ready.id, &error).await;
                    }
                    state.apply_cooldown(&ready.id, &capability, scope, Duration::from_secs(20));
                    state.unbind_route(route_key, &ready.id);
                    last_error = error;
                    break;
                }
                Err(_) => {
                    let error = format!(
                        "{}: 上游首个响应事件等待超过 {} 秒",
                        ready.name,
                        UPSTREAM_FIRST_EVENT_TIMEOUT.as_secs()
                    );
                    if let Some(log) = &attempt_log {
                        log.finish("error", Some(&error));
                    }
                    if !image_generation::is_dedicated_account(&ready) {
                        let _ = state.db.set_error_async(&ready.id, &error).await;
                    }
                    state.cool_down_account(&ready.id, Duration::from_secs(20));
                    state.unbind_route(route_key, &ready.id);
                    last_error = error;
                    break;
                }
            };

            if let Some(log) = &attempt_log {
                log.mark_response(status.as_u16());
            }

            if status.is_success() && capability.endpoint == EndpointFamily::Responses {
                if let Some(state_header) = upstream_turn_state {
                    let mut response_headers = HeaderMap::new();
                    response_headers.insert("x-codex-turn-state", state_header);
                    state.note_codex_turn_state(route_key, &ready, &response_headers);
                }
            }

            state.clear_cooldown(&ready.id, &capability);
            if attempt_started.elapsed() <= STICKY_ROUTE_MAX_FIRST_EVENT_LATENCY {
                state.bind_route(route_key, &ready.id);
            } else {
                state.unbind_route(route_key, &ready.id);
            }
            if !image_generation::is_dedicated_account(&ready) {
                let _ = state.db.mark_used_async(&ready.id).await;
            }
            if let Some(usage) = usage {
                let estimate = estimate_cost(&usage, model_hint.as_deref());
                if let Some(log) = &attempt_log {
                    log.record_usage(RequestLogUsage::from_breakdown(
                        &usage,
                        estimate.total_cost,
                        estimate.unpriced_tokens,
                    ));
                }
                if !image_generation::is_dedicated_account(&ready) {
                    if let Err(error) = state
                        .db
                        .record_usage_async(
                            &ready.id,
                            &usage,
                            estimate.total_cost,
                            estimate.unpriced_tokens,
                        )
                        .await
                    {
                        warn!(account_id = %ready.id, %error, "记录 Token 用量失败");
                    }
                }
            }
            if !completion_deferred {
                if let Some(log) = &attempt_log {
                    log.finish("success", None);
                }
            }
            return hold_capacity_lease(response, capacity_lease);
        }
    }

    if let Some((log, message)) = pending_retry.take() {
        log.finish("error", Some(&message));
    }
    if attempted_accounts == 0 {
        request_log.record_local_failure(StatusCode::SERVICE_UNAVAILABLE, &last_error);
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            &last_error,
            "capacity_exhausted",
        );
    }
    json_error(StatusCode::BAD_GATEWAY, &last_error, "upstream_error")
}

fn request_body_limit_message(limit: usize) -> String {
    const MEBIBYTE: usize = 1024 * 1024;
    if limit >= MEBIBYTE && limit % MEBIBYTE == 0 {
        format!(
            "request body exceeds the configured {} MB limit",
            limit / MEBIBYTE
        )
    } else {
        format!("request body exceeds the configured {limit} byte limit")
    }
}

fn request_body_limit_response(limit: usize) -> Response {
    json_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        &request_body_limit_message(limit),
        "invalid_request_error",
    )
}

pub(super) fn hold_capacity_lease(response: Response, lease: UpstreamCapacityLease) -> Response {
    let (parts, body) = response.into_parts();
    let stream = futures::stream::unfold(
        (body.into_data_stream(), lease),
        |(mut body, lease)| async move { body.next().await.map(|chunk| (chunk, (body, lease))) },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}

pub(super) fn cooling_down_response(retry_after: Option<u64>) -> Response {
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

pub(super) fn request_route_key(headers: &HeaderMap, body: &[u8]) -> Option<u64> {
    for header in [
        "x-session-id",
        "session-id",
        "session_id",
        "x-conversation-id",
        "conversation-id",
        "conversation_id",
        "x-prompt-cache-key",
        "prompt-cache-key",
    ] {
        if let Some(value) = headers
            .get(header)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(stable_route_hash(value));
        }
    }
    let value = serde_json::from_slice::<Value>(body).ok()?;
    for field in ["session_id", "conversation_id", "prompt_cache_key"] {
        if let Some(value) = value
            .get(field)
            .or_else(|| {
                value
                    .get("metadata")
                    .and_then(|metadata| metadata.get(field))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(stable_route_hash(value));
        }
    }
    None
}

pub(super) fn stable_route_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(super) fn account_supports_request(account: &Account, capability: &RequestCapability) -> bool {
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

pub(super) fn model_is_allowed(configured_models: &[String], requested_model: &str) -> bool {
    if configured_models.is_empty() {
        return true;
    }
    configured_models
        .iter()
        .any(|pattern| wildcard_model_matches(pattern, requested_model))
}

pub(super) fn wildcard_model_matches(pattern: &str, model: &str) -> bool {
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

pub(super) fn classify_failure(status: StatusCode, has_model: bool) -> FailurePolicy {
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

pub(super) fn classify_failure_with_content_type(
    status: StatusCode,
    has_model: bool,
    content_type: Option<&str>,
) -> FailurePolicy {
    if status == StatusCode::FORBIDDEN
        && content_type.is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
    {
        return FailurePolicy {
            switch_account: true,
            cooldown_scope: has_model.then_some(CooldownScope::Capability),
        };
    }
    classify_failure(status, has_model)
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    #[test]
    fn html_forbidden_is_capability_scoped_without_account_penalty() {
        let policy = classify_failure_with_content_type(
            StatusCode::FORBIDDEN,
            true,
            Some("text/html; charset=utf-8"),
        );
        assert!(policy.switch_account);
        assert_eq!(policy.cooldown_scope, Some(CooldownScope::Capability));
        let policy =
            classify_failure_with_content_type(StatusCode::FORBIDDEN, false, Some("text/html"));
        assert!(policy.switch_account);
        assert_eq!(policy.cooldown_scope, None);
    }
}

pub(super) fn persist_codex_quota_headers(
    state: &Arc<ProxyState>,
    account: &Account,
    headers: &reqwest::header::HeaderMap,
    force: bool,
) {
    if account.account_type != "oauth"
        || (!force && state.quota_snapshot_throttle.get(&account.id).is_some())
    {
        return;
    }
    let Some(entry) = quota::codex_header_cache_entry_json(account, headers) else {
        return;
    };
    state.quota_snapshot_throttle.insert(account.id.clone(), ());
    let db = Arc::clone(&state.db);
    let app_handle = state.app_handle.clone();
    let account_id = account.id.clone();
    let event_entry = serde_json::from_str::<Value>(&entry).ok();
    let entries = vec![(account_id.clone(), entry)];
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            match db
                .merge_json_setting_entries_async(quota::QUOTA_CACHE_KEY, entries)
                .await
            {
                Ok(()) => {
                    emit_quota_cache_update(app_handle.as_ref(), &account_id, event_entry.as_ref())
                }
                Err(error) => {
                    warn!(account_id = %account_id, %error, "保存响应头额度快照失败");
                }
            }
        });
    } else {
        match db.merge_json_setting_entries(quota::QUOTA_CACHE_KEY, &entries) {
            Ok(()) => {
                emit_quota_cache_update(app_handle.as_ref(), &account_id, event_entry.as_ref())
            }
            Err(error) => {
                warn!(account_id = %account_id, %error, "保存响应头额度快照失败");
            }
        }
    }
}

fn emit_quota_cache_update(
    app_handle: Option<&tauri::AppHandle>,
    account_id: &str,
    entry: Option<&Value>,
) {
    let (Some(app_handle), Some(entry)) = (app_handle, entry) else {
        return;
    };
    let _ = app_handle.emit(
        "quota-cache-updated",
        json!({"account_id": account_id, "entry": entry}),
    );
}

pub(super) fn parse_retry_after(
    value: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Duration> {
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

pub(super) fn response_cooldown(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> Duration {
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, chrono::Utc::now()))
            .unwrap_or_else(|| Duration::from_secs(30));
        // Cap the 429 cooldown so a single account recovers quickly; prefer
        // switching to other accounts instead of a long local lockout.
        return retry_after.min(Duration::from_secs(120));
    }
    match status.as_u16() {
        401..=403 => Duration::from_secs(300),
        404 => Duration::from_secs(10 * 60),
        408 => Duration::from_secs(20),
        _ if status.is_server_error() => Duration::from_secs(20),
        _ => Duration::from_secs(30),
    }
}
