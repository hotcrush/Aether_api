use super::*;
use crate::db::NewAccount;
use tower::ServiceExt;

fn test_state() -> ProxyState {
    ProxyState {
        db: Arc::new(Db::new(std::path::Path::new(":memory:")).unwrap()),
        client: Arc::new(arc_swap::ArcSwap::new(Arc::new(reqwest::Client::new()))),
        access_token: Arc::new(arc_swap::ArcSwap::new(Arc::new(String::new()))),
        cost_guard: Arc::new(arc_swap::ArcSwap::new(Arc::new(
            CostGuardSettings::default(),
        ))),
        codex_version: Arc::new(arc_swap::ArcSwap::new(Arc::new(
            crate::codex_identity::DEFAULT_CODEX_VERSION.to_string(),
        ))),
        app_handle: None,
        capacity: Arc::new(CapacityRegistry::default()),
        cooldowns: Cache::builder().time_to_live(COOLDOWN_CACHE_TTL).build(),
        stream_quarantines: Cache::builder().time_to_live(STREAM_QUARANTINE_TTL).build(),
        quota_snapshot_throttle: Cache::builder()
            .time_to_live(QUOTA_SNAPSHOT_THROTTLE)
            .build(),
        sticky_routes: Cache::builder()
            .time_to_idle(STICKY_ROUTE_TTL)
            .max_capacity(MAX_STICKY_ROUTES)
            .build(),
        weighted_schedules: Mutex::new(HashMap::new()),
        refresh_locks: Mutex::new(HashMap::new()),
        rate_limiter: RateLimiter::dashmap(Quota::per_second(
            NonZeroU32::new(DEFAULT_ACCOUNT_RPS).unwrap(),
        )),
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
        rate_multiplier: 1.0,
        auto_sync_rate_multiplier: false,
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
fn cost_guard_respects_rate_multiplier_and_safety_buffer() {
    let state = test_state();
    state.cost_guard.store(Arc::new(CostGuardSettings {
        enabled: true,
        max_cost_multiplier: 1.5,
        safety_buffer: 0.1,
    }));
    let mut allowed = scheduling_account("allowed", 1);
    allowed.rate_multiplier = 1.35;
    let mut excluded = scheduling_account("excluded", 1);
    excluded.rate_multiplier = 1.36;

    let (ordered, _) = state.ordered_accounts(
        vec![excluded, allowed],
        None,
        &responses_capability("gpt-5"),
    );

    assert_eq!(ordered.len(), 1);
    assert_eq!(ordered[0].id, "allowed");
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
    assert_eq!(
        value.pointer("/stream_options/include_usage"),
        Some(&json!(true))
    );
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
            request_log: None,
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
fn responses_subpaths_use_a_closed_safe_character_set() {
    for path in [
        "/responses",
        "/v1/responses/compact",
        "/backend-api/codex/responses/input_tokens",
        "/v1/responses/resp_123-abc/cancel",
    ] {
        assert!(is_forwardable_responses_path(path), "rejected {path}");
    }

    for path in [
        "/v1/responses/..",
        "/v1/responses/./cancel",
        "/v1/responses//compact",
        "/v1/responses/compact/",
        "/v1/responses/%2e%2e/admin",
        "/v1/responses/compact?embedded",
        "/v1/responses/中文",
    ] {
        assert!(!is_forwardable_responses_path(path), "accepted {path}");
    }

    let oversized_segment = format!("/v1/responses/{}", "a".repeat(129));
    assert!(!is_forwardable_responses_path(&oversized_segment));

    let too_many_segments = format!("/v1/responses/{}", ["a"; 9].join("/"));
    assert!(!is_forwardable_responses_path(&too_many_segments));
}

#[test]
fn upstream_url_builders_reject_unsafe_response_suffixes() {
    let uri: Uri = "/v1/responses/%2e%2e/admin".parse().unwrap();
    assert!(oauth_target_url(&uri, crate::codex_identity::DEFAULT_CODEX_VERSION).is_err());

    let mut relay = scheduling_account("relay", 1);
    relay.account_type = "api_key".to_string();
    relay.base_url = "https://relay.example/v1".to_string();
    assert!(api_key_target_url(&relay, &uri).is_err());
}

#[test]
fn oauth_models_url_rebuilds_client_version_from_the_effective_identity() {
    let uri: Uri = "/v1/models?client_version=0.100.0".parse().unwrap();
    assert_eq!(
        oauth_target_url(&uri, "0.147.0").unwrap(),
        "https://chatgpt.com/backend-api/codex/models?client_version=0.147.0"
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
fn sticky_session_stays_on_its_channel_across_priority_tiers() {
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
    assert_eq!(ordered, ["bound", "a", "b"]);
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
fn session_keys_are_shared_across_headers_and_json_fields() {
    let mut headers = HeaderMap::new();
    headers.insert("x-session-id", HeaderValue::from_static("session-42"));
    let header_key = request_route_key(&headers, b"{}").unwrap();

    for body in [
        br#"{"session_id":"session-42"}"#.as_slice(),
        br#"{"conversation_id":"session-42"}"#.as_slice(),
        br#"{"prompt_cache_key":"session-42"}"#.as_slice(),
        br#"{"metadata":{"session_id":"session-42"}}"#.as_slice(),
    ] {
        assert_eq!(request_route_key(&HeaderMap::new(), body), Some(header_key));
    }
}

#[test]
fn sticky_requests_do_not_advance_new_session_weighting() {
    let state = test_state();
    let capability = responses_capability("gpt-5");
    let accounts = vec![
        scheduling_account("a", 1),
        scheduling_account("b", 1),
        scheduling_account("bound", 5),
    ];

    assert_eq!(
        state.order_by_priority(accounts.clone(), None, &capability)[0].id,
        "a"
    );
    for _ in 0..3 {
        assert_eq!(
            state.order_by_priority(accounts.clone(), Some("bound"), &capability)[0].id,
            "bound"
        );
    }
    assert_eq!(
        state.order_by_priority(accounts, None, &capability)[0].id,
        "b"
    );
}

#[tokio::test]
async fn router_accepts_payloads_above_axums_default_limit() {
    let app = build_proxy_router(Arc::new(test_state()), MAX_PROXY_REQUEST_BODY_SIZE);
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/responses")
                .body(Body::from(vec![b'x'; 2 * 1024 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn router_rejects_unsafe_response_subpaths_before_loading_accounts() {
    let state = test_state();
    state
        .access_token
        .store(Arc::new("local-test-token".to_string()));
    let app = build_proxy_router(Arc::new(state), MAX_PROXY_REQUEST_BODY_SIZE);
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/responses/%2e%2e/admin")
                .header(header::AUTHORIZATION, "Bearer local-test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body.pointer("/error/type"), Some(&json!("not_found_error")));
}

#[tokio::test]
async fn router_returns_openai_error_above_configured_body_limit() {
    const TEST_LIMIT: usize = 1024;

    let known_length = build_proxy_router(Arc::new(test_state()), TEST_LIMIT)
        .oneshot(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/responses")
                .header(header::CONTENT_LENGTH, TEST_LIMIT + 1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(known_length.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let unknown_length = build_proxy_router(Arc::new(test_state()), TEST_LIMIT)
        .oneshot(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri("/v1/responses")
                .body(Body::from(vec![b'x'; TEST_LIMIT + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown_length.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = axum::body::to_bytes(unknown_length.into_body(), 4096)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body.pointer("/error/type"),
        Some(&json!("invalid_request_error"))
    );
    assert!(body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.contains("1024 byte limit")));
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
    let (ordered, retry_after) = state.ordered_accounts(vec![primary, backup], None, &capability);
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
    let first = state.cooldowns.get(&key).unwrap();
    state.cool_down_account("account", Duration::from_secs(20));
    let second = state.cooldowns.get(&key).unwrap();

    assert!(second >= first);
}

#[test]
fn stream_quarantine_prefers_healthy_accounts_and_fails_open_when_all_are_quarantined() {
    let state = test_state();
    let capability = responses_capability("gpt-5");
    let primary = scheduling_account("primary", 1);
    let backup = scheduling_account("backup", 1);

    state.quarantine_stream(&primary.id, &capability, CooldownScope::Account);
    let (ordered, _, fail_open) = state.ordered_accounts_for_request(
        vec![primary.clone(), backup.clone()],
        None,
        &capability,
    );
    assert!(!fail_open);
    assert_eq!(ordered[0].id, backup.id);

    state.quarantine_stream(&backup.id, &capability, CooldownScope::Account);
    let (ordered, retry_after, fail_open) = state.ordered_accounts_for_request(
        vec![primary.clone(), backup.clone()],
        None,
        &capability,
    );
    assert!(fail_open);
    assert_eq!(retry_after, None);
    assert_eq!(ordered.len(), 2);

    state.cool_down_account(&primary.id, Duration::from_secs(60));
    state.cool_down_account(&backup.id, Duration::from_secs(60));
    let (ordered, retry_after, fail_open) =
        state.ordered_accounts_for_request(vec![primary, backup], None, &capability);
    assert!(!fail_open);
    assert!(ordered.is_empty());
    assert!(retry_after.is_some());
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
    let (ordered, _) = state.ordered_accounts(vec![account_a, account_b.clone()], None, &model_two);
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
fn stream_load_shed_errors_keep_their_machine_code() {
    let value = json!({
        "type": "response.failed",
        "response": {
            "error": {
                "code": "server_is_overloaded",
                "message": "The server is overloaded"
            }
        }
    });
    let summary = stream_error_from_value(&value).unwrap();
    assert!(summary.contains("server_is_overloaded"));
    assert!(is_transient_load_shed_message(&summary));
    assert!(is_transient_load_shed_message("slow_down: retry later"));
    assert!(!is_transient_load_shed_message("quota exhausted"));
    let sse = b"event: error\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"slow_down\",\"message\":\"retry\"}}}\n\n";
    assert!(is_transient_load_shed_message(
        &stream_payload_error(sse).unwrap()
    ));
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

    let mut disconnected = Box::pin(futures::stream::iter(vec![Err::<Bytes, _>("disconnected")]));
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
    let later_error = Bytes::from_static(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"code\":\"slow_down\"}}\n\n");
    let mut committed = Box::pin(futures::stream::iter(vec![
        Ok::<_, &'static str>(Bytes::new()),
        Ok(first_payload.clone()),
        Ok(later_error.clone()),
    ]));
    let error = read_stream_bootstrap(committed.as_mut(), true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("slow_down"));

    let output = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
    );
    let mut ready = Box::pin(futures::stream::iter(vec![
        Ok::<_, &'static str>(first_payload),
        Ok(output.clone()),
    ]));
    let bootstrap = read_stream_bootstrap(ready.as_mut(), true).await.unwrap();
    assert_eq!(bootstrap, Bytes::from_static(
        b"data: {\"type\":\"response.created\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
    ));
}

#[test]
fn capacity_shed_errors_are_rewritten_only_for_client_output() {
    let event = b"event: error\ndata: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"busy\"}}\n\n";
    let rewritten = sanitize_capacity_shed_sse_event(event);
    assert!(std::str::from_utf8(&rewritten)
        .unwrap()
        .contains("\"code\":\"server_error\""));
    assert!(std::str::from_utf8(&rewritten).unwrap().contains("busy"));
    assert!(!std::str::from_utf8(&rewritten)
        .unwrap()
        .contains("server_is_overloaded"));
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
            request_log: None,
        },
        true,
    );

    observer.observe_chunk(
        b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5\",\"usage\":{\"input_tokens\":2,\"output_tokens\":3}}}\n\n",
    );
    assert_eq!(state.db.total_tokens().unwrap(), 5);
    assert!(state.sticky_routes.contains_key(&42));

    observer.observe_chunk(
        b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"late failure\"}}}\n\n",
    );
    let cooldown_key = capability.cooldown_key(&account.id).unwrap();
    assert!(state.cooldowns.contains_key(&cooldown_key));
    assert!(!state.stream_quarantines.contains_key(&cooldown_key));
    assert!(!state.sticky_routes.contains_key(&42));
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
            request_log: None,
        },
        true,
    );
    incomplete.observe_chunk(b"data: {\"type\":\"response.created\"}\n\n");
    incomplete.record_eof();
    let incomplete_key = incomplete_capability.cooldown_key(&account.id).unwrap();
    assert!(state.stream_quarantines.contains_key(&incomplete_key));
    assert!(!state.cooldowns.contains_key(&incomplete_key));
    assert!(!state.sticky_routes.contains_key(&43));
}

#[tokio::test]
async fn oauth_sse_to_json_keeps_quota_and_rate_limit_headers() {
    let upstream = reqwest::Response::from(
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(reqwest::header::CONTENT_TYPE, "text/event-stream")
            .header("x-codex-primary-used-percent", "42")
            .header("x-ratelimit-remaining-tokens", "149984")
            .body(reqwest::Body::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\"}}\n\n",
            ))
            .unwrap(),
    );

    let (response, _, completion_deferred) = to_client_response(upstream, true, false, None)
        .await
        .unwrap();
    assert!(!completion_deferred);
    assert_eq!(
        response
            .headers()
            .get("x-codex-primary-used-percent")
            .unwrap(),
        "42"
    );
    assert_eq!(
        response
            .headers()
            .get("x-ratelimit-remaining-tokens")
            .unwrap(),
        "149984"
    );
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
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
