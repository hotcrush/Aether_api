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

trait WebSocketIo: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> WebSocketIo for T {}

type BoxedWebSocketIo = Box<dyn WebSocketIo + Unpin + Send>;
type UpstreamWebSocket = WebSocketStream<MaybeTlsStream<BoxedWebSocketIo>>;

struct ConnectedWebSocket {
    account: Account,
    _capacity_lease: CapacityLease,
    socket: UpstreamWebSocket,
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
    let request_log = ProxyRequestLogContext::new(&state, &method, &uri, &capability, true);
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
        let mut connected =
            connect_selected_upstream(&state, &uri, &headers, &first_request).await?;
        connected
            .socket
            .send(UpstreamMessage::Text(first_request.clone().into()))
            .await
            .map_err(|error| format!("发送首个 WebSocket 请求失败: {error}"))?;
        relay_websockets(
            &mut client,
            &mut connected.socket,
            &state,
            &uri,
            &connected.account,
            connected.route_key,
            connected.observer,
        )
        .await
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
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("Codex WebSocket 请求不是有效 JSON: {error}"))?;
    let message_type = value.get("type").and_then(Value::as_str);
    if message_type == Some("response.cancel") && allow_cancel {
        return Ok(text.to_string());
    }
    if message_type != Some("response.create") {
        return Err("Codex WebSocket 请求类型必须是 response.create".to_string());
    }
    let normalized = sanitize_responses_tool_parameter_types(text.as_bytes()).0;
    String::from_utf8(normalized).map_err(|_| "Codex WebSocket 请求不是有效 UTF-8 文本".to_string())
}

async fn connect_selected_upstream(
    state: &Arc<ProxyState>,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    first_request: &str,
) -> Result<ConnectedWebSocket, String> {
    let capability = RequestCapability::from_request(uri, first_request.as_bytes());
    let request_log = ProxyRequestLogContext::new(state, &Method::POST, uri, &capability, true);
    let accounts = state
        .db
        .get_active_accounts_async()
        .await
        .map_err(|error| format!("读取账号失败: {error}"))?
        .into_iter()
        .filter(|account| account_supports_request(account, &capability))
        .collect::<Vec<_>>();
    if accounts.is_empty() {
        request_log
            .record_local_failure(StatusCode::SERVICE_UNAVAILABLE, "没有支持该模型的可用账号");
        return Err("没有支持该模型的可用账号".to_string());
    }

    let route_key = request_route_key(inbound_headers, first_request.as_bytes());
    let (accounts, _, _) = state.ordered_accounts_for_request(accounts, route_key, &capability);
    let startup_deadline = tokio::time::Instant::now() + REQUEST_STARTUP_BUDGET;
    let outbound_proxy = crate::outbound_proxy::load(&state.db);
    let mut last_error = "所有上游 WebSocket 连接均失败".to_string();
    let mut attempts = 0usize;

    for account in accounts {
        if attempts >= MAX_ACCOUNT_ATTEMPTS || tokio::time::Instant::now() >= startup_deadline {
            break;
        }
        let Some(capacity_lease) = state.capacity.try_acquire(&account.id, account.concurrency)
        else {
            last_error = "所有匹配账号均已达到并发上限".to_string();
            continue;
        };
        if !state.cost_guard.load().allows(account.rate_multiplier) {
            last_error = "所有匹配账号均被成本保护排除".to_string();
            continue;
        }

        attempts += 1;
        let attempt_log = request_log.begin_attempt(Some(&account), attempts as i64);
        let ready = match tokio::time::timeout_at(
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

        let codex_version = crate::codex_identity::current_version(&state.codex_version);
        let connected = tokio::time::timeout_at(
            startup_deadline,
            connect_upstream_websocket(
                &ready,
                uri,
                inbound_headers,
                &codex_version,
                &outbound_proxy,
            ),
        )
        .await;
        let (socket, response_headers) = match connected {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => {
                if let Some(log) = &attempt_log {
                    log.finish("retry", Some(&error));
                }
                let _ = state.db.set_error_async(&ready.id, &error).await;
                state.cool_down_account(&ready.id, Duration::from_secs(20));
                state.unbind_route(route_key, &ready.id);
                last_error = error;
                continue;
            }
            Err(_) => {
                let error = "WebSocket 上游连接超时".to_string();
                if let Some(log) = &attempt_log {
                    log.finish("error", Some(&error));
                }
                last_error = error;
                break;
            }
        };

        if let Some(log) = &attempt_log {
            log.mark_response(StatusCode::SWITCHING_PROTOCOLS.as_u16());
        }
        persist_codex_quota_headers(state, &ready, &response_headers, false);
        state.clear_cooldown(&ready.id, &capability);
        state.bind_route(route_key, &ready.id);
        let _ = state.db.mark_used_async(&ready.id).await;
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
            _capacity_lease: capacity_lease,
            socket,
            observer,
            route_key,
        });
    }

    Err(last_error)
}

async fn connect_upstream_websocket(
    account: &Account,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    codex_version: &str,
    outbound_proxy: &crate::outbound_proxy::OutboundProxySettings,
) -> Result<(UpstreamWebSocket, HeaderMap), String> {
    let target = websocket_target_url(account, uri, codex_version)?;
    let request =
        websocket_upstream_request(target.as_str(), account, inbound_headers, codex_version)?;
    let stream = tokio::time::timeout(
        WEBSOCKET_CONNECT_TIMEOUT,
        connect_websocket_transport(&target, outbound_proxy),
    )
    .await
    .map_err(|_| "连接 WebSocket 出站代理超时".to_string())??;
    let (socket, response) = client_async_tls_with_config(request, stream, None, None)
        .await
        .map_err(websocket_connect_error)?;
    Ok((socket, response.headers().clone()))
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
        "session_id",
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

fn websocket_connect_error(error: tokio_tungstenite::tungstenite::Error) -> String {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            format!("WebSocket 上游握手失败: {}", response.status())
        }
        error => format!("WebSocket 上游连接失败: {error}"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn relay_websockets(
    client: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    state: &Arc<ProxyState>,
    uri: &Uri,
    account: &Account,
    route_key: Option<u64>,
    first_observer: StreamBodyObserver,
) -> Result<(), String> {
    let mut observer = Some(first_observer);
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
                        let text = normalize_websocket_request(text.as_str(), true)?;
                        if message_type == Some("response.create") {
                            if let Some(previous) = observer.as_mut() {
                                previous.record_transport_failure("新的 response.create 在上一响应完成前到达");
                            }
                            observer = Some(new_websocket_observer(
                                state,
                                uri,
                                account,
                                route_key,
                                text.as_bytes(),
                            ));
                        }
                        upstream.send(UpstreamMessage::Text(text.into())).await
                            .map_err(|error| format!("转发 Codex WebSocket 请求失败: {error}"))?;
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
                    if let Some(observer) = observer.as_mut() {
                        observer.record_transport_failure("上游 WebSocket 在响应完成前断开");
                    }
                    return Err("上游 WebSocket 已断开".to_string());
                };
                let upstream_message = upstream_message
                    .map_err(|error| format!("读取上游 WebSocket 失败: {error}"))?;
                match upstream_message {
                    UpstreamMessage::Text(text) => {
                        let terminal = stream_has_terminal_event(text.as_bytes());
                        let failed = stream_payload_error(text.as_bytes()).is_some();
                        if let Some(active) = observer.as_mut() {
                            active.observe_event(text.as_bytes());
                        }
                        client.send(ClientMessage::Text(text.to_string().into())).await
                            .map_err(|error| format!("向 Codex 转发 WebSocket 事件失败: {error}"))?;
                        if terminal || failed {
                            observer = None;
                        }
                    }
                    UpstreamMessage::Binary(data) => {
                        client.send(ClientMessage::Binary(data)).await
                            .map_err(|error| format!("向 Codex 转发 WebSocket 二进制帧失败: {error}"))?;
                    }
                    UpstreamMessage::Ping(data) => {
                        upstream.send(UpstreamMessage::Pong(data)).await
                            .map_err(|error| format!("回复上游 WebSocket 心跳失败: {error}"))?;
                    }
                    UpstreamMessage::Pong(_) | UpstreamMessage::Frame(_) => {}
                    UpstreamMessage::Close(_) => {
                        let _ = client.send(ClientMessage::Close(None)).await;
                        return Ok(());
                    }
                }
            }
        }
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
    let request_log = ProxyRequestLogContext::new(state, &Method::POST, uri, &capability, true)
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
