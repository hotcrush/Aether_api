use super::*;
use axum::extract::ws::{Message as ClientMessage, WebSocket};
use axum::http::header::AUTHORIZATION;
use futures::SinkExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;
use tokio_tungstenite::{client_async_tls_with_config, MaybeTlsStream, WebSocketStream};

const RESPONSES_WEBSOCKETS_BETA: &str = "responses_websockets=2026-02-06";
const WEBSOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const FIRST_WEBSOCKET_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_BRIDGE_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const WEBSOCKET_HTTP_BRIDGE_THRESHOLD_BYTES: usize = 15 * 1024 * 1024;
const MAX_WEBSOCKET_START_ATTEMPTS: usize = 2;

trait WebSocketIo: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> WebSocketIo for T {}

type BoxedWebSocketIo = Box<dyn WebSocketIo + Unpin + Send>;
type UpstreamWebSocket = WebSocketStream<MaybeTlsStream<BoxedWebSocketIo>>;
type UpstreamHttpStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>;

struct PreparedHttpBridgeTurn {
    stream: UpstreamHttpStream,
    bootstrap: Bytes,
    response_headers: reqwest::header::HeaderMap,
    status: u16,
}

enum ConnectedTransport {
    WebSocket {
        socket: UpstreamWebSocket,
        first_messages: Vec<UpstreamMessage>,
    },
    HttpBridge(PreparedHttpBridgeTurn),
}

struct ConnectedWebSocket {
    account: Account,
    capacity_lease: UpstreamCapacityLease,
    transport: ConnectedTransport,
    observer: StreamBodyObserver,
    route_key: Option<u64>,
}

pub(super) async fn websocket_upgrade_response(
    state: Arc<ProxyState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response {
    let capability = RequestCapability::from_request(&uri, &[]);
    let request_log =
        ProxyRequestLogContext::new(&state, &method, &uri, &capability, true, "websocket");
    if method != Method::GET
        || !is_responses_path(uri.path())
        || !response_path_suffix(uri.path()).is_empty()
    {
        request_log
            .record_local_failure(StatusCode::NOT_FOUND, "WebSocket 仅支持 Responses 主端点");
        return json_error(
            StatusCode::NOT_FOUND,
            "WebSocket 仅支持 Responses 主端点",
            "not_found_error",
        );
    }
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

    websocket
        .max_message_size(MAX_PROXY_REQUEST_BODY_SIZE)
        .max_frame_size(MAX_PROXY_REQUEST_BODY_SIZE)
        .on_upgrade(move |socket| run_websocket_proxy(socket, state, uri, headers))
        .into_response()
}

async fn run_websocket_proxy(
    mut client: WebSocket,
    state: Arc<ProxyState>,
    uri: Uri,
    headers: HeaderMap,
) {
    let result = async {
        let first_request = receive_first_websocket_request(&mut client).await?;
        let connected = connect_selected_upstream(&state, &uri, &headers, &first_request).await?;
        let ConnectedWebSocket {
            account,
            capacity_lease,
            transport,
            observer,
            route_key,
        } = connected;
        match transport {
            ConnectedTransport::WebSocket {
                mut socket,
                first_messages,
            } => {
                relay_websockets(
                    &mut client,
                    &mut socket,
                    &state,
                    &uri,
                    &headers,
                    &account,
                    route_key,
                    first_messages,
                    observer,
                    capacity_lease,
                )
                .await
            }
            ConnectedTransport::HttpBridge(prepared) => {
                relay_http_bridge(
                    &mut client,
                    &state,
                    &uri,
                    &headers,
                    &account,
                    route_key,
                    prepared,
                    observer,
                    capacity_lease,
                )
                .await
            }
        }
    }
    .await;

    if let Err(error) = result {
        warn!(%error, "Codex WebSocket 代理连接已终止");
        let _ = send_websocket_error(&mut client, StatusCode::BAD_GATEWAY, &error).await;
        let _ = client.send(ClientMessage::Close(None)).await;
    }
}

async fn receive_first_websocket_request(client: &mut WebSocket) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + FIRST_WEBSOCKET_REQUEST_TIMEOUT;
    loop {
        let message = tokio::time::timeout_at(deadline, client.recv())
            .await
            .map_err(|_| "等待首个 response.create 超时".to_string())?
            .ok_or_else(|| "Codex 在发送首个 response.create 前关闭了连接".to_string())?
            .map_err(|error| format!("读取 Codex WebSocket 请求失败: {error}"))?;
        match message {
            ClientMessage::Text(text) => return normalize_websocket_request(text.as_str(), false),
            ClientMessage::Ping(_) | ClientMessage::Pong(_) => continue,
            ClientMessage::Close(_) => {
                return Err("Codex 在发送首个 response.create 前关闭了连接".to_string())
            }
            ClientMessage::Binary(_) => {
                return Err("Codex WebSocket 首个请求必须是 JSON 文本".to_string())
            }
        }
    }
}

fn normalize_websocket_request(text: &str, allow_cancel: bool) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(text)
        .map_err(|error| format!("Codex WebSocket 请求不是有效 JSON: {error}"))?;
    let message_type = value.get("type").and_then(Value::as_str);
    if message_type == Some("response.cancel") && allow_cancel {
        return Ok(text.to_string());
    }
    if message_type != Some("response.create") {
        return Err("Codex WebSocket 请求类型必须是 response.create".to_string());
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| "response.create 必须是 JSON 对象".to_string())?;
    sanitize_responses_tool_parameter_types_in_object(object);
    serde_json::to_string(&value).map_err(|error| format!("序列化 WebSocket 请求失败: {error}"))
}

fn normalize_oauth_websocket_request(text: &str) -> Result<String, String> {
    let mut value: Value = serde_json::from_str(text)
        .map_err(|error| format!("Codex WebSocket 请求不是有效 JSON: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "response.create 必须是 JSON 对象".to_string())?;
    match object.get("truncation").and_then(Value::as_str) {
        Some("disabled") => {
            object.remove("truncation");
        }
        Some("auto") => {
            return Err(
                "OAuth Responses 不支持 truncation=auto，请删除该字段或改用 truncation=disabled"
                    .to_string(),
            );
        }
        Some(other) => return Err(format!("不支持的 truncation 值: {other}")),
        None => {}
    }
    serde_json::to_string(&value).map_err(|error| format!("序列化 WebSocket 请求失败: {error}"))
}

async fn connect_selected_upstream(
    state: &Arc<ProxyState>,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    first_request: &str,
) -> Result<ConnectedWebSocket, String> {
    let capability = RequestCapability::from_request(uri, first_request.as_bytes());
    let request_log =
        ProxyRequestLogContext::new(state, &Method::POST, uri, &capability, true, "websocket");
    let image_settings = state.image_generation.load_full();
    let dedicated_image = capability.image_generation && image_settings.enabled;
    let accounts = if dedicated_image {
        vec![image_generation::dedicated_account(&image_settings)]
    } else {
        state
            .db
            .get_active_accounts_async()
            .await
            .map_err(|error| format!("读取账号失败: {error}"))?
            .into_iter()
            .filter(|account| account_supports_request(account, &capability))
            .collect::<Vec<_>>()
    };
    if accounts.is_empty() {
        request_log
            .record_local_failure(StatusCode::SERVICE_UNAVAILABLE, "没有支持该模型的可用账号");
        return Err("没有支持该模型的可用账号".to_string());
    }

    let route_key = request_route_key(inbound_headers, first_request.as_bytes());
    if first_request.len() >= LARGE_REQUEST_WARNING_BYTES {
        warn!(
            request_bytes = first_request.len(),
            route_key,
            "WebSocket response.create 超过 15 MiB，将通过 HTTP SSE 桥接并保留完整上下文"
        );
    }
    let (accounts, _, _) = state.ordered_accounts_for_request(accounts, route_key, &capability);
    let startup_deadline = tokio::time::Instant::now() + REQUEST_STARTUP_BUDGET;
    let outbound_proxy = crate::outbound_proxy::load(&state.db);
    let mut last_error = "所有上游连接均失败".to_string();
    let mut attempts = 0usize;

    for account in accounts {
        if attempts >= MAX_ACCOUNT_ATTEMPTS || tokio::time::Instant::now() >= startup_deadline {
            break;
        }
        let Some(capacity_lease) = state.try_acquire_capacity(&account) else {
            last_error = "所有匹配账号均已达到并发上限".to_string();
            continue;
        };
        if !dedicated_image && !state.cost_guard.load().allows(account.rate_multiplier) {
            last_error = "所有匹配账号均被成本保护排除".to_string();
            continue;
        }

        attempts += 1;
        let attempt_started = tokio::time::Instant::now();
        let use_http_bridge = should_use_http_bridge(&account, first_request.len());
        let attempt_request_log = request_log.with_transport(
            if use_http_bridge {
                "websocket_http_bridge"
            } else {
                "websocket"
            },
            !use_http_bridge,
        );
        let attempt_log = attempt_request_log.begin_attempt(Some(&account), attempts as i64);
        let mut ready = match tokio::time::timeout_at(
            startup_deadline,
            ensure_account_ready(state, &account, false),
        )
        .await
        {
            Ok(Ok(account)) => account,
            Ok(Err(error)) => {
                if let Some(log) = &attempt_log {
                    log.finish("retry", Some(&error));
                }
                last_error = error;
                continue;
            }
            Err(_) => {
                last_error = "WebSocket 连接准备超时".to_string();
                break;
            }
        };

        if use_http_bridge {
            let prepared = tokio::time::timeout(
                HTTP_BRIDGE_STARTUP_TIMEOUT,
                prepare_http_bridge_turn(state, &ready, uri, inbound_headers, first_request),
            )
            .await;
            let prepared = match prepared {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => {
                    let message = format!("{}: {error}", ready.name);
                    if let Some(log) = &attempt_log {
                        log.finish("retry", Some(&message));
                    }
                    if !error.is_transient_load_shed() {
                        if !image_generation::is_dedicated_account(&ready) {
                            let _ = state.db.set_error_async(&ready.id, &message).await;
                        }
                        state.apply_cooldown(
                            &ready.id,
                            &capability,
                            error.cooldown_scope(),
                            Duration::from_secs(20),
                        );
                    }
                    state.unbind_route(route_key, &ready.id);
                    last_error = message;
                    continue;
                }
                Err(_) => {
                    let message = format!(
                        "{}: HTTP SSE 上游连接或首事件等待超时（120 秒）",
                        ready.name
                    );
                    if let Some(log) = &attempt_log {
                        log.finish("retry", Some(&message));
                    }
                    state.unbind_route(route_key, &ready.id);
                    last_error = message;
                    continue;
                }
            };

            if let Some(log) = &attempt_log {
                log.mark_response(prepared.status);
            }
            persist_codex_quota_headers(state, &ready, &prepared.response_headers, false);
            state.clear_cooldown(&ready.id, &capability);
            if attempt_started.elapsed() <= STICKY_ROUTE_MAX_FIRST_EVENT_LATENCY {
                state.bind_route(route_key, &ready.id);
            } else {
                state.unbind_route(route_key, &ready.id);
            }
            if !image_generation::is_dedicated_account(&ready) {
                let _ = state.db.mark_used_async(&ready.id).await;
            }
            let observer = StreamBodyObserver::new(
                StreamObserverContext {
                    state: Arc::clone(state),
                    account_id: ready.id.clone(),
                    capability,
                    route_key,
                    model_hint: extract_model_hint(first_request.as_bytes()),
                    request_log: attempt_log,
                },
                true,
            );
            return Ok(ConnectedWebSocket {
                account: ready,
                capacity_lease,
                transport: ConnectedTransport::HttpBridge(prepared),
                observer,
                route_key,
            });
        }

        let codex_version = crate::codex_identity::current_version(&state.codex_version);
        let first_request = if ready.account_type == "oauth" {
            normalize_oauth_websocket_request(first_request)?
        } else {
            first_request.to_string()
        };
        let prepared_fingerprint = crate::codex_fingerprint::prepare(
            &ready,
            state.codex_fingerprint.load().mode,
            inbound_headers,
            first_request.as_bytes(),
        );
        let upstream_headers = prepared_fingerprint.headers;
        let first_request = String::from_utf8(prepared_fingerprint.body)
            .map_err(|error| format!("序列化 Codex 指纹请求失败: {error}"))?;
        let mut socket = None;
        let mut first_messages = None;
        let mut response_headers = HeaderMap::new();
        let mut start_error: Option<PrepareResponseError> = None;
        let mut upstream_upgraded = false;
        let mut refreshed_after_unauthorized = false;
        for start_attempt in 1..=MAX_WEBSOCKET_START_ATTEMPTS {
            let connected = tokio::time::timeout_at(
                startup_deadline,
                connect_upstream_websocket(
                    &ready,
                    uri,
                    &upstream_headers,
                    &codex_version,
                    &outbound_proxy,
                ),
            )
            .await;
            let (mut candidate, candidate_headers) = match connected {
                Ok(Ok(connected)) => {
                    upstream_upgraded = true;
                    connected
                }
                Ok(Err(PrepareResponseError::Unauthorized(error)))
                    if ready.account_type == "oauth"
                        && !ready.refresh_token.is_empty()
                        && !refreshed_after_unauthorized =>
                {
                    refreshed_after_unauthorized = true;
                    match tokio::time::timeout_at(
                        startup_deadline,
                        ensure_account_ready(state, &ready, true),
                    )
                    .await
                    {
                        Ok(Ok(refreshed)) => {
                            ready = refreshed;
                            continue;
                        }
                        Ok(Err(refresh_error)) => {
                            start_error = Some(PrepareResponseError::Unauthorized(format!(
                                "{error}; OAuth Token 刷新失败: {refresh_error}"
                            )));
                            break;
                        }
                        Err(_) => {
                            start_error = Some(PrepareResponseError::Unauthorized(
                                "OAuth Token 刷新超时".to_string(),
                            ));
                            break;
                        }
                    }
                }
                Ok(Err(error)) => {
                    start_error = Some(error);
                    if start_attempt < MAX_WEBSOCKET_START_ATTEMPTS
                        && tokio::time::Instant::now() < startup_deadline
                    {
                        continue;
                    }
                    break;
                }
                Err(_) => {
                    start_error = Some(PrepareResponseError::Transport(
                        "WebSocket 上游连接超时".to_string(),
                    ));
                    break;
                }
            };
            let first_frame = tokio::time::timeout_at(
                startup_deadline,
                candidate.send(UpstreamMessage::Text(first_request.clone().into())),
            )
            .await;
            if let Some(error) = match first_frame {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("发送首个 WebSocket 请求失败: {error}")),
                Err(_) => Some("发送首个 WebSocket 请求超时".to_string()),
            } {
                start_error = Some(PrepareResponseError::Transport(error.clone()));
                if start_attempt < MAX_WEBSOCKET_START_ATTEMPTS
                    && tokio::time::Instant::now() < startup_deadline
                {
                    warn!(
                        account_id = %ready.id,
                        attempt = start_attempt,
                        %error,
                        "Codex WebSocket 首帧发送失败，重新建立上游连接"
                    );
                    continue;
                }
                break;
            }
            let first_event_deadline = std::cmp::min(
                startup_deadline,
                tokio::time::Instant::now() + UPSTREAM_FIRST_EVENT_TIMEOUT,
            );
            let message = match read_websocket_bootstrap(&mut candidate, first_event_deadline).await
            {
                Ok(message) => message,
                Err(error) => {
                    let retry_transport = matches!(error, PrepareResponseError::Transport(_))
                        && start_attempt < MAX_WEBSOCKET_START_ATTEMPTS
                        && tokio::time::Instant::now() < startup_deadline;
                    if retry_transport {
                        warn!(
                            account_id = %ready.id,
                            attempt = start_attempt,
                            %error,
                            "Codex WebSocket 首个上游事件失败，重新建立连接"
                        );
                    }
                    start_error = Some(error);
                    if retry_transport {
                        continue;
                    }
                    break;
                }
            };
            socket = Some(candidate);
            first_messages = Some(message);
            response_headers = candidate_headers;
            start_error = None;
            break;
        }
        let (socket, first_messages, response_headers) = match (socket, first_messages, start_error)
        {
            (Some(socket), Some(first_messages), _) => (socket, first_messages, response_headers),
            (None, None, Some(error)) => {
                let message = format!("{}: {error}", ready.name);
                if let Some(log) = &attempt_log {
                    if upstream_upgraded {
                        log.mark_response(StatusCode::SWITCHING_PROTOCOLS.as_u16());
                    }
                    log.finish("retry", Some(&message));
                }
                if !error.is_transient_load_shed() {
                    if !image_generation::is_dedicated_account(&ready) {
                        let _ = state.db.set_error_async(&ready.id, &message).await;
                    }
                    state.apply_cooldown(
                        &ready.id,
                        &capability,
                        error.cooldown_scope(),
                        Duration::from_secs(20),
                    );
                }
                state.unbind_route(route_key, &ready.id);
                last_error = message;
                continue;
            }
            _ => {
                let error = "WebSocket 上游连接均未建立".to_string();
                if let Some(log) = &attempt_log {
                    log.finish("retry", Some(&error));
                }
                last_error = error;
                continue;
            }
        };

        if let Some(log) = &attempt_log {
            log.mark_response(StatusCode::SWITCHING_PROTOCOLS.as_u16());
        }
        persist_codex_quota_headers(state, &ready, &response_headers, false);
        state.clear_cooldown(&ready.id, &capability);
        if attempt_started.elapsed() <= STICKY_ROUTE_MAX_FIRST_EVENT_LATENCY {
            state.bind_route(route_key, &ready.id);
        } else {
            state.unbind_route(route_key, &ready.id);
        }
        if !image_generation::is_dedicated_account(&ready) {
            let _ = state.db.mark_used_async(&ready.id).await;
        }
        let observer = StreamBodyObserver::new(
            StreamObserverContext {
                state: Arc::clone(state),
                account_id: ready.id.clone(),
                capability,
                route_key,
                model_hint: extract_model_hint(first_request.as_bytes()),
                request_log: attempt_log,
            },
            false,
        );
        return Ok(ConnectedWebSocket {
            account: ready,
            capacity_lease,
            transport: ConnectedTransport::WebSocket {
                socket,
                first_messages,
            },
            observer,
            route_key,
        });
    }

    Err(last_error)
}

fn should_use_http_bridge(account: &Account, request_bytes: usize) -> bool {
    let base_url = account.base_url.trim();
    let parsed_url = reqwest::Url::parse(base_url).ok();
    if parsed_url
        .as_ref()
        .is_some_and(|url| matches!(url.scheme(), "ws" | "wss"))
    {
        return false;
    }
    if request_bytes >= WEBSOCKET_HTTP_BRIDGE_THRESHOLD_BYTES {
        return true;
    }
    if account.account_type == "oauth" {
        return false;
    }
    if base_url.is_empty() {
        return false;
    }
    let Some(url) = parsed_url else {
        return true;
    };
    !url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.openai.com"))
}

fn prepare_http_bridge_body(request: &str) -> Result<Vec<u8>, String> {
    let mut value: Value = serde_json::from_str(request)
        .map_err(|error| format!("Codex WebSocket 请求不是有效 JSON: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "response.create 必须是 JSON 对象".to_string())?;
    object.remove("type");
    object.remove("generate");
    object.insert("stream".to_string(), Value::Bool(true));
    serde_json::to_vec(&value).map_err(|error| format!("生成 HTTP SSE 请求失败: {error}"))
}

async fn prepare_http_bridge_turn(
    state: &Arc<ProxyState>,
    account: &Account,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    request: &str,
) -> Result<PreparedHttpBridgeTurn, PrepareResponseError> {
    let body = prepare_http_bridge_body(request).map_err(PrepareResponseError::Upstream)?;
    let mut headers = inbound_headers.clone();
    headers.remove("openai-beta");
    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("text/event-stream"),
    );
    let codex_version = crate::codex_identity::current_version(&state.codex_version);
    let client = state.client.load_full();
    let mut ready = account.clone();
    let mut refreshed_after_unauthorized = false;
    let response = loop {
        let response = send_upstream(
            client.as_ref(),
            &ready,
            &Method::POST,
            uri,
            &headers,
            &body,
            &codex_version,
            state.codex_fingerprint.load().mode,
        )
        .await
        .map_err(|error| PrepareResponseError::Transport(error.to_string()))?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED
            && ready.account_type == "oauth"
            && !ready.refresh_token.is_empty()
            && !refreshed_after_unauthorized
        {
            refreshed_after_unauthorized = true;
            ready = ensure_account_ready(state, &ready, true)
                .await
                .map_err(PrepareResponseError::Unauthorized)?;
            continue;
        }
        if !status.is_success() {
            let summary = upstream_error_summary(response).await;
            return Err(if status == StatusCode::UNAUTHORIZED {
                PrepareResponseError::Unauthorized(summary)
            } else {
                PrepareResponseError::Upstream(summary)
            });
        }
        break response;
    };
    let status = response.status();
    let response_headers = response.headers().clone();
    let mut stream: UpstreamHttpStream = Box::pin(response.bytes_stream());
    let bootstrap = read_stream_bootstrap(stream.as_mut(), true).await?;
    Ok(PreparedHttpBridgeTurn {
        stream,
        bootstrap,
        response_headers,
        status: status.as_u16(),
    })
}

async fn connect_upstream_websocket(
    account: &Account,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    codex_version: &str,
    outbound_proxy: &crate::outbound_proxy::OutboundProxySettings,
) -> Result<(UpstreamWebSocket, HeaderMap), PrepareResponseError> {
    let target = websocket_target_url(account, uri, codex_version)
        .map_err(PrepareResponseError::Transport)?;
    let request =
        websocket_upstream_request(target.as_str(), account, inbound_headers, codex_version)
            .map_err(PrepareResponseError::Transport)?;
    let stream = tokio::time::timeout(
        WEBSOCKET_CONNECT_TIMEOUT,
        connect_websocket_transport(&target, outbound_proxy),
    )
    .await
    .map_err(|_| PrepareResponseError::Transport("连接 WebSocket 出站代理超时".to_string()))?
    .map_err(PrepareResponseError::Transport)?;
    let (socket, response) = client_async_tls_with_config(request, stream, None, None)
        .await
        .map_err(websocket_connect_error)?;
    Ok((socket, response.headers().clone()))
}

async fn read_websocket_bootstrap(
    socket: &mut UpstreamWebSocket,
    deadline: tokio::time::Instant,
) -> Result<Vec<UpstreamMessage>, PrepareResponseError> {
    let mut buffered = Vec::new();
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| {
                PrepareResponseError::Transport("等待 WebSocket 首个上游事件超时".to_string())
            })?
            .ok_or_else(|| {
                PrepareResponseError::Transport("上游 WebSocket 在首个响应事件前断开".to_string())
            })?
            .map_err(|error| {
                PrepareResponseError::Transport(format!("读取 WebSocket 首个上游事件失败: {error}"))
            })?;
        match message {
            UpstreamMessage::Text(text) => {
                if let Some(error) = stream_payload_error(text.as_bytes()) {
                    return Err(PrepareResponseError::Upstream(error));
                }
                if stream_has_empty_completed_event(text.as_bytes())
                    && !stream_has_semantic_output(text.as_bytes())
                    && !stream_has_usage(text.as_bytes())
                    && !buffered.iter().any(|message| {
                        matches!(message, UpstreamMessage::Text(text) if stream_has_semantic_output(text.as_bytes()))
                    })
                    && !buffered.iter().any(|message| {
                        matches!(message, UpstreamMessage::Text(text) if stream_has_usage(text.as_bytes()))
                    })
                {
                    return Err(PrepareResponseError::Upstream(
                        "upstream returned an empty response.completed event".to_string(),
                    ));
                }
                let ready = stream_has_semantic_output(text.as_bytes())
                    || stream_has_terminal_event(text.as_bytes());
                buffered.push(UpstreamMessage::Text(text));
                if ready {
                    return Ok(buffered);
                }
            }
            UpstreamMessage::Binary(data) => {
                buffered.push(UpstreamMessage::Binary(data));
                return Ok(buffered);
            }
            UpstreamMessage::Ping(data) => {
                socket
                    .send(UpstreamMessage::Pong(data))
                    .await
                    .map_err(|error| {
                        PrepareResponseError::Transport(format!(
                            "回复 WebSocket 启动心跳失败: {error}"
                        ))
                    })?
            }
            UpstreamMessage::Pong(_) | UpstreamMessage::Frame(_) => {}
            UpstreamMessage::Close(frame) => {
                return Err(PrepareResponseError::Upstream(format!(
                    "上游 WebSocket 在首个响应事件前关闭: {}",
                    upstream_close_detail(frame.as_ref())
                )));
            }
        }
    }
}

fn websocket_target_url(
    account: &Account,
    uri: &Uri,
    codex_version: &str,
) -> Result<reqwest::Url, String> {
    let target = if account.account_type == "oauth" {
        oauth_target_url(uri, codex_version)?
    } else {
        api_key_target_url(account, uri)?
    };
    let mut url =
        reqwest::Url::parse(&target).map_err(|error| format!("WebSocket 上游地址无效: {error}"))?;
    let websocket_scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => return Ok(url),
        scheme => return Err(format!("WebSocket 上游不支持 {scheme} 协议")),
    };
    url.set_scheme(websocket_scheme)
        .map_err(|_| "无法转换 WebSocket 上游协议".to_string())?;
    Ok(url)
}

fn websocket_upstream_request(
    target: &str,
    account: &Account,
    inbound_headers: &HeaderMap,
    codex_version: &str,
) -> Result<axum::http::Request<()>, String> {
    let mut request = target
        .into_client_request()
        .map_err(|error| format!("创建 WebSocket 握手请求失败: {error}"))?;
    let headers = request.headers_mut();
    for name in [
        "accept-language",
        "conversation_id",
        "session-id",
        "session_id",
        "thread-id",
        "x-client-request-id",
        "x-codex-beta-features",
        "x-codex-installation-id",
        "x-codex-parent-thread-id",
        "x-codex-routing-hint",
        "x-codex-turn-state",
        "x-codex-turn-metadata",
        "x-codex-window-id",
        "x-openai-subagent",
        "x-responsesapi-include-timing-metrics",
        "openai-organization",
        "openai-project",
        "traceparent",
    ] {
        if let Some(value) = inbound_headers.get(name) {
            headers.insert(name, value.clone());
        }
    }
    if let Some(value) = inbound_headers.get("openai-beta") {
        headers.insert("openai-beta", value.clone());
    } else {
        headers.insert(
            "openai-beta",
            HeaderValue::from_static(RESPONSES_WEBSOCKETS_BETA),
        );
    }
    if let Some(value) = inbound_headers.get("user-agent") {
        headers.insert("user-agent", value.clone());
    } else {
        headers.insert(
            "user-agent",
            HeaderValue::from_str(&crate::codex_identity::user_agent(codex_version))
                .map_err(|error| format!("Codex User-Agent 无效: {error}"))?,
        );
    }
    for (name, fallback) in [("originator", "codex-tui"), ("version", codex_version)] {
        if let Some(value) = inbound_headers.get(name) {
            headers.insert(name, value.clone());
        } else {
            headers.insert(
                name,
                HeaderValue::from_str(fallback)
                    .map_err(|error| format!("Codex 身份头无效: {error}"))?,
            );
        }
    }

    let secret = if account.account_type == "oauth" {
        &account.access_token
    } else {
        &account.api_key
    };
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {secret}"))
            .map_err(|error| format!("上游授权头无效: {error}"))?,
    );
    if account.account_type == "oauth" && !account.chatgpt_account_id.is_empty() {
        headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_str(&account.chatgpt_account_id)
                .map_err(|error| format!("ChatGPT 账号头无效: {error}"))?,
        );
    }
    Ok(request)
}

async fn connect_websocket_transport(
    target: &reqwest::Url,
    settings: &crate::outbound_proxy::OutboundProxySettings,
) -> Result<BoxedWebSocketIo, String> {
    let target_host = target
        .host_str()
        .ok_or_else(|| "WebSocket 上游缺少主机名".to_string())?;
    let target_port = target
        .port_or_known_default()
        .ok_or_else(|| "WebSocket 上游缺少端口".to_string())?;
    if !settings.enabled {
        let stream = TcpStream::connect((target_host, target_port))
            .await
            .map_err(|error| format!("连接 WebSocket 上游失败: {error}"))?;
        return Ok(Box::new(stream));
    }

    let proxy =
        reqwest::Url::parse(&settings.url).map_err(|error| format!("出站代理地址无效: {error}"))?;
    if !proxy.username().is_empty() || proxy.password().is_some() {
        return Err("Codex WebSocket 暂不支持带账号密码的出站代理".to_string());
    }
    let proxy_host = proxy
        .host_str()
        .ok_or_else(|| "出站代理缺少主机名".to_string())?;
    let stream: BoxedWebSocketIo = match proxy.scheme() {
        "http" => {
            let proxy_port = proxy.port().unwrap_or(80);
            Box::new(connect_http_proxy(proxy_host, proxy_port, target_host, target_port).await?)
        }
        "socks5h" => {
            let proxy_port = proxy.port().unwrap_or(1080);
            Box::new(
                Socks5Stream::connect((proxy_host, proxy_port), (target_host, target_port))
                    .await
                    .map_err(|error| format!("通过 FlClash SOCKS5H 连接失败: {error}"))?,
            )
        }
        "socks5" => {
            let proxy_port = proxy.port().unwrap_or(1080);
            let target_address = tokio::net::lookup_host((target_host, target_port))
                .await
                .map_err(|error| format!("解析 WebSocket 上游地址失败: {error}"))?
                .next()
                .ok_or_else(|| "WebSocket 上游 DNS 没有返回地址".to_string())?;
            Box::new(
                Socks5Stream::connect((proxy_host, proxy_port), target_address)
                    .await
                    .map_err(|error| format!("通过 FlClash SOCKS5 连接失败: {error}"))?,
            )
        }
        "https" => {
            return Err(
                "Codex WebSocket 暂不支持 HTTPS 出站代理，请使用 HTTP、SOCKS5 或 SOCKS5H 地址"
                    .to_string(),
            );
        }
        scheme => return Err(format!("Codex WebSocket 不支持 {scheme} 出站代理")),
    };
    Ok(stream)
}

async fn connect_http_proxy(
    proxy_host: &str,
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, String> {
    const MAX_CONNECT_RESPONSE_SIZE: usize = 16 * 1024;
    let mut stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(|error| format!("连接 HTTP 出站代理失败: {error}"))?;
    let authority = host_port(target_host, target_port);
    let request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("发送 HTTP CONNECT 请求失败: {error}"))?;

    let mut response = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    while !response.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("读取 HTTP CONNECT 响应失败: {error}"))?;
        if read == 0 {
            return Err("HTTP 出站代理在 CONNECT 完成前关闭了连接".to_string());
        }
        response.extend_from_slice(&buffer[..read]);
        if response.len() > MAX_CONNECT_RESPONSE_SIZE {
            return Err("HTTP CONNECT 响应头超过 16 KiB".to_string());
        }
    }

    let status_line = response
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .map(str::trim)
        .ok_or_else(|| "HTTP CONNECT 响应无效".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| format!("HTTP CONNECT 状态行无效: {status_line}"))?;
    if status != StatusCode::OK.as_u16() {
        return Err(format!("HTTP 出站代理拒绝 CONNECT: {status_line}"));
    }
    Ok(stream)
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn websocket_connect_error(error: tokio_tungstenite::tungstenite::Error) -> PrepareResponseError {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            let message = format!("WebSocket 上游握手失败: {}", response.status());
            if response.status() == StatusCode::UNAUTHORIZED {
                PrepareResponseError::Unauthorized(message)
            } else {
                PrepareResponseError::Upstream(message)
            }
        }
        error => PrepareResponseError::Transport(format!("WebSocket 上游连接失败: {error}")),
    }
}

enum HttpBridgeTurnEnd {
    Completed,
    Cancelled,
    ClientClosed,
    UpstreamFailed,
}

#[allow(clippy::too_many_arguments)]
async fn relay_http_bridge(
    client: &mut WebSocket,
    state: &Arc<ProxyState>,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    account: &Account,
    route_key: Option<u64>,
    mut prepared: PreparedHttpBridgeTurn,
    mut observer: StreamBodyObserver,
    first_capacity_lease: UpstreamCapacityLease,
) -> Result<(), String> {
    let mut capacity_lease = Some(first_capacity_lease);
    loop {
        match relay_http_bridge_turn(client, &mut prepared, &mut observer).await? {
            HttpBridgeTurnEnd::ClientClosed | HttpBridgeTurnEnd::UpstreamFailed => return Ok(()),
            HttpBridgeTurnEnd::Completed | HttpBridgeTurnEnd::Cancelled => {}
        }
        capacity_lease.take();

        let Some(next_request) = receive_next_http_bridge_request(client).await? else {
            return Ok(());
        };
        let capability = RequestCapability::from_request(uri, next_request.as_bytes());
        if !account_supports_request(account, &capability) {
            send_websocket_error(
                client,
                StatusCode::BAD_REQUEST,
                "当前 WebSocket 上游不支持新的请求模型",
            )
            .await
            .map_err(|error| format!("发送 WebSocket 模型错误失败: {error}"))?;
            return Ok(());
        }
        let Some(next_capacity_lease) = state.try_acquire_capacity(account) else {
            send_websocket_error(
                client,
                StatusCode::TOO_MANY_REQUESTS,
                "当前中转站正在处理其他会话，请稍后重试",
            )
            .await
            .map_err(|error| format!("发送 WebSocket 容量错误失败: {error}"))?;
            continue;
        };
        let request_log =
            ProxyRequestLogContext::new(state, &Method::POST, uri, &capability, true, "websocket")
                .with_transport("websocket_http_bridge", false);
        let attempt_log = request_log.begin_attempt(Some(account), 1);
        let next_prepared = tokio::time::timeout(
            HTTP_BRIDGE_STARTUP_TIMEOUT,
            prepare_http_bridge_turn(state, account, uri, inbound_headers, &next_request),
        )
        .await;
        prepared = match next_prepared {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(error)) => {
                let message = format!("{}: {error}", account.name);
                if let Some(log) = &attempt_log {
                    log.finish("error", Some(&message));
                }
                if !error.is_transient_load_shed() {
                    state.apply_cooldown(
                        &account.id,
                        &capability,
                        error.cooldown_scope(),
                        Duration::from_secs(20),
                    );
                }
                state.unbind_route(route_key, &account.id);
                send_websocket_error(client, StatusCode::BAD_GATEWAY, &message)
                    .await
                    .map_err(|send_error| format!("发送 HTTP SSE 桥接错误失败: {send_error}"))?;
                return Ok(());
            }
            Err(_) => {
                let message = format!(
                    "{}: HTTP SSE 上游连接或首事件等待超时（120 秒）",
                    account.name
                );
                if let Some(log) = &attempt_log {
                    log.finish("error", Some(&message));
                }
                state.unbind_route(route_key, &account.id);
                send_websocket_error(client, StatusCode::GATEWAY_TIMEOUT, &message)
                    .await
                    .map_err(|send_error| format!("发送 HTTP SSE 桥接超时失败: {send_error}"))?;
                return Ok(());
            }
        };
        if let Some(log) = &attempt_log {
            log.mark_response(prepared.status);
        }
        persist_codex_quota_headers(state, account, &prepared.response_headers, false);
        state.clear_cooldown(&account.id, &capability);
        state.bind_route(route_key, &account.id);
        observer = StreamBodyObserver::new(
            StreamObserverContext {
                state: Arc::clone(state),
                account_id: account.id.clone(),
                capability,
                route_key,
                model_hint: extract_model_hint(next_request.as_bytes()),
                request_log: attempt_log,
            },
            true,
        );
        capacity_lease = Some(next_capacity_lease);
    }
}

async fn relay_http_bridge_turn(
    client: &mut WebSocket,
    prepared: &mut PreparedHttpBridgeTurn,
    observer: &mut StreamBodyObserver,
) -> Result<HttpBridgeTurnEnd, String> {
    let mut buffer = std::mem::take(&mut prepared.bootstrap).to_vec();
    loop {
        while let Some(event_end) = next_sse_event_end(&buffer) {
            let event = buffer.drain(..event_end).collect::<Vec<_>>();
            let terminal = stream_has_terminal_event(&event);
            let failed = stream_payload_error(&event).is_some();
            observer.observe_event(&event);
            let sanitized = sanitize_capacity_shed_sse_event(&event);
            if let Some(payload) = sse_event_payload(&sanitized)? {
                client
                    .send(ClientMessage::Text(payload.into()))
                    .await
                    .map_err(|error| format!("向 Codex 转发 HTTP SSE 桥接事件失败: {error}"))?;
            }
            if terminal {
                return Ok(HttpBridgeTurnEnd::Completed);
            }
            if failed {
                return Ok(HttpBridgeTurnEnd::UpstreamFailed);
            }
        }

        tokio::select! {
            client_message = client.recv() => {
                let Some(client_message) = client_message else {
                    observer.record_transport_failure("Codex 客户端断开 WebSocket");
                    return Ok(HttpBridgeTurnEnd::ClientClosed);
                };
                let client_message = client_message
                    .map_err(|error| format!("读取 Codex WebSocket 失败: {error}"))?;
                match client_message {
                    ClientMessage::Text(text) => {
                        let value: Value = serde_json::from_str(text.as_str())
                            .map_err(|error| format!("Codex WebSocket 请求不是有效 JSON: {error}"))?;
                        match value.get("type").and_then(Value::as_str) {
                            Some("response.cancel") => {
                                let event = json!({
                                    "type": "response.cancelled",
                                    "response": { "status": "cancelled" }
                                }).to_string();
                                observer.observe_event(event.as_bytes());
                                client.send(ClientMessage::Text(event.into())).await
                                    .map_err(|error| format!("发送 WebSocket 取消事件失败: {error}"))?;
                                return Ok(HttpBridgeTurnEnd::Cancelled);
                            }
                            Some("response.create") => {
                                observer.record_transport_failure(
                                    "新的 response.create 在上一响应完成前到达",
                                );
                                return Err("新的 response.create 在上一响应完成前到达".to_string());
                            }
                            _ => return Err("Codex WebSocket 请求类型无效".to_string()),
                        }
                    }
                    ClientMessage::Ping(_) | ClientMessage::Pong(_) => {}
                    ClientMessage::Close(_) => {
                        observer.record_transport_failure("Codex 客户端关闭 WebSocket");
                        return Ok(HttpBridgeTurnEnd::ClientClosed);
                    }
                    ClientMessage::Binary(_) => {
                        return Err("Codex WebSocket 请求必须是 JSON 文本".to_string());
                    }
                }
            }
            upstream_chunk = prepared.stream.next() => {
                match upstream_chunk {
                    Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                    Some(Err(error)) => {
                        observer.record_transport_failure(&format!("读取 HTTP SSE 上游失败: {error}"));
                        return Err(format!("读取 HTTP SSE 上游失败: {error}"));
                    }
                    None => {
                        observer.record_eof();
                        return Err("HTTP SSE 上游在 response.completed 前结束".to_string());
                    }
                }
            }
        }
        if buffer.len() > MAX_STREAM_BOOTSTRAP_BYTES {
            observer.record_transport_failure("HTTP SSE 单个事件超过 8 MiB");
            return Err("HTTP SSE 单个事件超过 8 MiB".to_string());
        }
    }
}

async fn receive_next_http_bridge_request(
    client: &mut WebSocket,
) -> Result<Option<String>, String> {
    loop {
        let Some(message) = client.recv().await else {
            return Ok(None);
        };
        let message = message.map_err(|error| format!("读取 Codex WebSocket 请求失败: {error}"))?;
        match message {
            ClientMessage::Text(text) => {
                let normalized = normalize_websocket_request(text.as_str(), true)?;
                let value: Value = serde_json::from_str(&normalized)
                    .map_err(|error| format!("Codex WebSocket 请求不是有效 JSON: {error}"))?;
                if value.get("type").and_then(Value::as_str) == Some("response.cancel") {
                    continue;
                }
                return Ok(Some(normalized));
            }
            ClientMessage::Ping(_) | ClientMessage::Pong(_) => continue,
            ClientMessage::Close(_) => return Ok(None),
            ClientMessage::Binary(_) => {
                return Err("Codex WebSocket 请求必须是 JSON 文本".to_string())
            }
        }
    }
}

fn sse_event_payload(event: &[u8]) -> Result<Option<String>, String> {
    let text = std::str::from_utf8(event)
        .map_err(|error| format!("HTTP SSE 上游返回了非 UTF-8 数据: {error}"))?;
    let normalized = text.replace("\r\n", "\n");
    let data = normalized
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str::<Value>(data)
        .map_err(|error| format!("HTTP SSE data 不是有效 JSON: {error}"))?;
    Ok(Some(data.to_string()))
}

#[allow(clippy::too_many_arguments)]
async fn relay_websockets(
    client: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    state: &Arc<ProxyState>,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    account: &Account,
    route_key: Option<u64>,
    first_messages: Vec<UpstreamMessage>,
    first_observer: StreamBodyObserver,
    first_capacity_lease: UpstreamCapacityLease,
) -> Result<(), String> {
    let mut observer = Some(first_observer);
    let mut capacity_lease = Some(first_capacity_lease);
    let mut first_event_deadline = None;
    for first_message in first_messages {
        match first_message {
            UpstreamMessage::Text(text) => {
                let terminal = stream_has_terminal_event(text.as_bytes());
                if let Some(active) = observer.as_mut() {
                    active.observe_event(text.as_bytes());
                }
                client
                    .send(ClientMessage::Text(text.to_string().into()))
                    .await
                    .map_err(|error| format!("向 Codex 转发 WebSocket 首个事件失败: {error}"))?;
                if terminal {
                    observer = None;
                    capacity_lease.take();
                }
            }
            UpstreamMessage::Binary(data) => {
                client
                    .send(ClientMessage::Binary(data))
                    .await
                    .map_err(|error| {
                        format!("向 Codex 转发 WebSocket 首个二进制帧失败: {error}")
                    })?;
            }
            _ => return Err("WebSocket 启动阶段返回了无效首帧".to_string()),
        }
    }
    loop {
        tokio::select! {
            client_message = client.recv() => {
                let Some(client_message) = client_message else {
                    if let Some(observer) = observer.as_mut() {
                        observer.record_transport_failure("Codex 客户端断开 WebSocket");
                    }
                    return Ok(());
                };
                let client_message = client_message
                    .map_err(|error| format!("读取 Codex WebSocket 失败: {error}"))?;
                match client_message {
                    ClientMessage::Text(text) => {
                        let value: Value = serde_json::from_str(text.as_str())
                            .map_err(|error| format!("Codex WebSocket 请求不是有效 JSON: {error}"))?;
                        let message_type = value.get("type").and_then(Value::as_str);
                        let mut text = normalize_websocket_request(text.as_str(), true)?;
                        if account.account_type == "oauth"
                            && message_type == Some("response.create")
                        {
                            text = normalize_oauth_websocket_request(&text)?;
                        }
                        if message_type == Some("response.create") {
                            let prepared = crate::codex_fingerprint::prepare(
                                account,
                                state.codex_fingerprint.load().mode,
                                inbound_headers,
                                text.as_bytes(),
                            );
                            text = String::from_utf8(prepared.body)
                                .map_err(|error| format!("序列化 Codex 指纹请求失败: {error}"))?;
                        }
                        if message_type == Some("response.create") {
                            if observer.is_some() {
                                send_websocket_error(
                                    client,
                                    StatusCode::CONFLICT,
                                    "上一轮响应尚未完成，不能开始新的 response.create",
                                )
                                .await
                                .map_err(|error| format!("发送 WebSocket 并发轮次错误失败: {error}"))?;
                                continue;
                            }
                            let Some(next_capacity_lease) = state.try_acquire_capacity(account) else {
                                send_websocket_error(
                                    client,
                                    StatusCode::TOO_MANY_REQUESTS,
                                    "当前中转站正在处理其他会话，请稍后重试",
                                )
                                .await
                                .map_err(|error| format!("发送 WebSocket 容量错误失败: {error}"))?;
                                continue;
                            };
                            observer = Some(new_websocket_observer(
                                state,
                                uri,
                                account,
                                route_key,
                                text.as_bytes(),
                            ));
                            capacity_lease = Some(next_capacity_lease);
                            first_event_deadline = Some(
                                tokio::time::Instant::now() + UPSTREAM_FIRST_EVENT_TIMEOUT,
                            );
                        }
                        upstream.send(UpstreamMessage::Text(text.into())).await
                            .map_err(|error| format!("转发 Codex WebSocket 请求失败: {error}"))?;
                        if message_type == Some("response.cancel") {
                            if let Some(active) = observer.as_mut() {
                                active.record_client_cancelled();
                            }
                            observer = None;
                            capacity_lease.take();
                            first_event_deadline = None;
                        }
                    }
                    ClientMessage::Binary(data) => {
                        upstream.send(UpstreamMessage::Binary(data)).await
                            .map_err(|error| format!("转发 Codex WebSocket 二进制帧失败: {error}"))?;
                    }
                    ClientMessage::Ping(_) | ClientMessage::Pong(_) => {}
                    ClientMessage::Close(_) => {
                        let _ = upstream.send(UpstreamMessage::Close(None)).await;
                        return Ok(());
                    }
                }
            }
            upstream_message = upstream.next() => {
                let Some(upstream_message) = upstream_message else {
                    if observer.is_some() {
                        if let Some(observer) = observer.as_mut() {
                            observer.record_transport_failure("上游 WebSocket 在响应完成前断开");
                        }
                        return Err("上游 WebSocket 已断开".to_string());
                    }
                    return Ok(());
                };
                let upstream_message = upstream_message
                    .map_err(|error| format!("读取上游 WebSocket 失败: {error}"))?;
                match upstream_message {
                    UpstreamMessage::Text(text) => {
                        first_event_deadline = None;
                        let terminal = stream_has_terminal_event(text.as_bytes());
                        let failed = stream_payload_error(text.as_bytes()).is_some();
                        if let Some(active) = observer.as_mut() {
                            active.observe_event(text.as_bytes());
                        }
                        client.send(ClientMessage::Text(text.to_string().into())).await
                            .map_err(|error| format!("向 Codex 转发 WebSocket 事件失败: {error}"))?;
                        if terminal || failed {
                            observer = None;
                            capacity_lease.take();
                        }
                    }
                    UpstreamMessage::Binary(data) => {
                        first_event_deadline = None;
                        client.send(ClientMessage::Binary(data)).await
                            .map_err(|error| format!("向 Codex 转发 WebSocket 二进制帧失败: {error}"))?;
                    }
                    UpstreamMessage::Ping(data) => {
                        upstream.send(UpstreamMessage::Pong(data)).await
                            .map_err(|error| format!("回复上游 WebSocket 心跳失败: {error}"))?;
                    }
                    UpstreamMessage::Pong(_) | UpstreamMessage::Frame(_) => {}
                    UpstreamMessage::Close(frame) => {
                        let detail = upstream_close_detail(frame.as_ref());
                        if observer.is_some() {
                            if let Some(active) = observer.as_mut() {
                                active.record_transport_failure(&format!(
                                    "上游 WebSocket 在响应完成前关闭: {detail}"
                                ));
                            }
                            return Err(format!("上游 WebSocket 已关闭: {detail}"));
                        }
                        let _ = client.send(ClientMessage::Close(None)).await;
                        return Ok(());
                    }
                }
            }
            _ = async {
                if let Some(deadline) = first_event_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                if let Some(active) = observer.as_mut() {
                    active.record_transport_failure("上游 WebSocket 首个响应事件等待超时");
                }
                capacity_lease.take();
                send_websocket_error(
                    client,
                    StatusCode::GATEWAY_TIMEOUT,
                    "上游 WebSocket 首个响应事件等待超时，请重试当前会话",
                )
                .await
                .map_err(|error| format!("发送 WebSocket 首事件超时失败: {error}"))?;
                return Ok(());
            }
        }
    }
}

fn upstream_close_detail(
    frame: Option<&tokio_tungstenite::tungstenite::protocol::CloseFrame>,
) -> String {
    let Some(frame) = frame else {
        return "无关闭帧".to_string();
    };
    let reason = frame.reason.trim();
    if reason.is_empty() {
        format!("code {}", frame.code)
    } else {
        format!("code {}, reason {}", frame.code, reason)
    }
}

fn new_websocket_observer(
    state: &Arc<ProxyState>,
    uri: &Uri,
    account: &Account,
    route_key: Option<u64>,
    body: &[u8],
) -> StreamBodyObserver {
    let capability = RequestCapability::from_request(uri, body);
    let request_log =
        ProxyRequestLogContext::new(state, &Method::POST, uri, &capability, true, "websocket")
            .begin_attempt(Some(account), 1);
    if let Some(log) = &request_log {
        log.mark_response(StatusCode::SWITCHING_PROTOCOLS.as_u16());
    }
    StreamBodyObserver::new(
        StreamObserverContext {
            state: Arc::clone(state),
            account_id: account.id.clone(),
            capability,
            route_key,
            model_hint: extract_model_hint(body),
            request_log,
        },
        false,
    )
}

async fn send_websocket_error(
    client: &mut WebSocket,
    status: StatusCode,
    message: &str,
) -> Result<(), axum::Error> {
    let message = message.chars().take(500).collect::<String>();
    client
        .send(ClientMessage::Text(
            json!({
                "type": "error",
                "status": status.as_u16(),
                "error": {
                    "type": "server_error",
                    "code": "aether_websocket_proxy_error",
                    "message": message,
                }
            })
            .to_string()
            .into(),
        ))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_responses_urls_to_websocket_urls() {
        let oauth = scheduling_account_for_websocket_test("oauth");
        assert_eq!(
            websocket_target_url(
                &oauth,
                &"/v1/responses".parse().unwrap(),
                crate::codex_identity::DEFAULT_CODEX_VERSION,
            )
            .unwrap()
            .as_str(),
            "wss://chatgpt.com/backend-api/codex/responses"
        );

        let mut relay = scheduling_account_for_websocket_test("relay");
        relay.account_type = "api_key".to_string();
        relay.base_url = "https://relay.example/v1".to_string();
        assert_eq!(
            websocket_target_url(
                &relay,
                &"/v1/responses".parse().unwrap(),
                crate::codex_identity::DEFAULT_CODEX_VERSION,
            )
            .unwrap()
            .as_str(),
            "wss://relay.example/v1/responses"
        );
    }

    #[test]
    fn validates_response_create_and_cancel_frames() {
        assert!(normalize_websocket_request(
            r#"{"type":"response.create","model":"gpt-5.6"}"#,
            false
        )
        .is_ok());
        assert!(normalize_websocket_request(r#"{"type":"response.cancel"}"#, false).is_err());
        assert!(normalize_websocket_request(r#"{"type":"response.cancel"}"#, true).is_ok());
    }

    #[test]
    fn bridges_custom_relays_but_keeps_official_openai_websockets() {
        let mut relay = scheduling_account_for_websocket_test("relay");
        relay.account_type = "api_key".to_string();
        relay.base_url = "https://relay.example/v1".to_string();
        assert!(should_use_http_bridge(&relay, 128));

        relay.base_url = "https://api.openai.com/v1".to_string();
        assert!(!should_use_http_bridge(&relay, 128));

        relay.base_url = "wss://relay.example/v1".to_string();
        assert!(!should_use_http_bridge(
            &relay,
            WEBSOCKET_HTTP_BRIDGE_THRESHOLD_BYTES
        ));
    }

    #[test]
    fn bridges_large_official_websocket_requests_over_http() {
        let oauth = scheduling_account_for_websocket_test("oauth");
        assert!(!should_use_http_bridge(
            &oauth,
            WEBSOCKET_HTTP_BRIDGE_THRESHOLD_BYTES - 1
        ));
        assert!(should_use_http_bridge(
            &oauth,
            WEBSOCKET_HTTP_BRIDGE_THRESHOLD_BYTES
        ));

        let mut openai = scheduling_account_for_websocket_test("openai");
        openai.account_type = "api_key".to_string();
        openai.base_url = "https://api.openai.com/v1".to_string();
        assert!(should_use_http_bridge(
            &openai,
            WEBSOCKET_HTTP_BRIDGE_THRESHOLD_BYTES
        ));
    }

    #[test]
    fn converts_response_create_into_http_responses_body() {
        let body = prepare_http_bridge_body(
            r#"{"type":"response.create","generate":true,"model":"gpt-5.6","stream":false,"previous_response_id":"resp_1"}"#,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert!(value.get("type").is_none());
        assert!(value.get("generate").is_none());
        assert_eq!(value.get("stream"), Some(&Value::Bool(true)));
        assert_eq!(
            value.get("previous_response_id").and_then(Value::as_str),
            Some("resp_1")
        );
    }

    #[test]
    fn formats_http_connect_authorities() {
        assert_eq!(host_port("chatgpt.com", 443), "chatgpt.com:443");
        assert_eq!(host_port("2001:db8::1", 443), "[2001:db8::1]:443");
    }

    fn scheduling_account_for_websocket_test(id: &str) -> Account {
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
            priority: 1,
            status: "active".to_string(),
            last_error: String::new(),
            last_used_at: None,
            request_count: 0,
            created_at: id.to_string(),
            updated_at: id.to_string(),
        }
    }
}
