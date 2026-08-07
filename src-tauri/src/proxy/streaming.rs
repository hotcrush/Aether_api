use super::*;

pub(super) struct StreamObserverContext {
    pub(super) state: Arc<ProxyState>,
    pub(super) account_id: String,
    pub(super) capability: RequestCapability,
    pub(super) route_key: Option<u64>,
    pub(super) model_hint: Option<String>,
    pub(super) request_log: Option<RequestLogHandle>,
}

impl StreamObserverContext {
    pub(super) fn record_failure(&self, scope: CooldownScope, message: &str) {
        self.record_failure_with_policy(scope, message, false);
    }

    pub(super) fn record_disconnect(&self, scope: CooldownScope, message: &str) {
        self.record_failure_with_policy(scope, message, true);
    }

    pub(super) fn record_transient_failure(&self, message: &str) {
        self.state.unbind_route(self.route_key, &self.account_id);
        if let Some(log) = &self.request_log {
            log.finish("error", Some(message));
        }
    }

    fn record_failure_with_policy(
        &self,
        scope: CooldownScope,
        message: &str,
        stream_disconnect: bool,
    ) {
        // Use async DB write if a tokio runtime is available, otherwise fall back to sync
        if tokio::runtime::Handle::try_current().is_ok() {
            let db = Arc::clone(&self.state.db);
            let account_id = self.account_id.clone();
            let message_owned = message.to_owned();
            tokio::spawn(async move {
                let _ = db.set_error_async(&account_id, &message_owned).await;
            });
        } else {
            let _ = self.state.db.set_error(&self.account_id, message);
        }
        if stream_disconnect {
            self.state
                .quarantine_stream(&self.account_id, &self.capability, scope);
        } else {
            self.state.apply_cooldown(
                &self.account_id,
                &self.capability,
                scope,
                Duration::from_secs(20),
            );
        }
        self.state.unbind_route(self.route_key, &self.account_id);
        if let Some(log) = &self.request_log {
            log.finish("error", Some(message));
        }
    }

    pub(super) fn record_usage(&self, usage: UsageBreakdown) {
        let estimate = estimate_cost(&usage, self.model_hint.as_deref());
        if let Some(log) = &self.request_log {
            log.record_usage(RequestLogUsage::from_breakdown(
                &usage,
                estimate.total_cost,
                estimate.unpriced_tokens,
            ));
        }
        // Use async DB write if a tokio runtime is available, otherwise fall back to sync
        if tokio::runtime::Handle::try_current().is_ok() {
            let db = Arc::clone(&self.state.db);
            let account_id = self.account_id.clone();
            let total_cost = estimate.total_cost;
            let unpriced_tokens = estimate.unpriced_tokens;
            tokio::spawn(async move {
                if let Err(error) = db
                    .record_usage_async(&account_id, &usage, total_cost, unpriced_tokens)
                    .await
                {
                    warn!(account_id = %account_id, %error, "记录 Token 用量失败");
                }
            });
        } else if let Err(error) = self.state.db.record_usage(
            &self.account_id,
            &usage,
            estimate.total_cost,
            estimate.unpriced_tokens,
        ) {
            warn!(account_id = %self.account_id, %error, "记录 Token 用量失败");
        }
    }

    pub(super) fn record_success(&self) {
        if let Some(log) = &self.request_log {
            log.finish("success", None);
        }
    }

    pub(super) fn record_cancelled(&self) {
        if let Some(log) = &self.request_log {
            log.finish("cancelled", Some("上游取消了响应"));
        }
    }
}

pub(super) struct StreamBodyObserver {
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
    pub(super) fn new(context: StreamObserverContext, sse: bool) -> Self {
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

    pub(super) fn observe_chunk(&mut self, chunk: &[u8]) {
        self.capture_tail.extend_from_slice(chunk);
        if self.observed_model.is_none() {
            self.observed_model = extract_string_field_from_fragment(&self.capture_tail, "model");
        }
        if self.observed_service_tier.is_none() {
            self.observed_service_tier =
                extract_string_field_from_fragment(&self.capture_tail, "service_tier");
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
        if self.buffer.len() > MAX_STREAM_OBSERVER_EVENT_BYTES {
            let discard = self.buffer.len() - MAX_STREAM_OBSERVER_EVENT_BYTES;
            self.buffer.drain(..discard);
        }
    }

    pub(super) fn observe_event(&mut self, event: &[u8]) {
        let terminal = stream_has_terminal_event(event);
        let cancelled = stream_has_cancelled_event(event);
        if terminal {
            self.terminal_seen = true;
        }
        if !self.failure_recorded {
            if let Some(error) = stream_payload_error(event) {
                self.failure_recorded = true;
                let message = format!("upstream stream failed after commit: {error}");
                if is_transient_load_shed_message(&error) {
                    self.context.record_transient_failure(&message);
                } else {
                    self.context
                        .record_failure(CooldownScope::Capability, &message);
                }
            }
        }
        if !self.usage_recorded {
            if let Ok(text) = std::str::from_utf8(event) {
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
        }
        if cancelled && !self.failure_recorded {
            self.failure_recorded = true;
            self.context.record_cancelled();
        } else if terminal && !self.failure_recorded {
            self.context.record_success();
        }
    }

    pub(super) fn record_transport_failure(&mut self, error: &str) {
        if self.failure_recorded {
            return;
        }
        self.failure_recorded = true;
        self.context.record_disconnect(
            CooldownScope::Account,
            &format!("upstream stream transport failed after commit: {error}"),
        );
    }

    pub(super) fn record_eof(&mut self) {
        if self.sse
            && self.context.capability.endpoint == EndpointFamily::Responses
            && !self.terminal_seen
            && !self.failure_recorded
        {
            self.failure_recorded = true;
            self.context.record_disconnect(
                CooldownScope::Capability,
                "upstream stream ended after commit without a terminal event",
            );
            return;
        }
        if !self.failure_recorded {
            self.context.record_success();
        }
    }
}

pub(super) async fn to_client_response(
    response: reqwest::Response,
    oauth_account: bool,
    requested_stream: bool,
    stream_observer: Option<StreamObserverContext>,
) -> Result<(Response, Option<UsageBreakdown>, bool), PrepareResponseError> {
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
        let mut client_response = json_response(status, completed);
        *client_response.headers_mut() = headers;
        client_response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        return Ok((client_response, usage, false));
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
        return Ok((resp, usage, false));
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
        return Ok((builder.body(Body::from_stream(stream)).unwrap(), None, true));
    }

    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let mut builder = Response::builder().status(status);
    *builder.headers_mut().unwrap() = headers;
    Ok((
        builder.body(Body::from_stream(stream)).unwrap(),
        None,
        false,
    ))
}

pub(super) async fn read_stream_bootstrap<S, E>(
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

pub(super) fn sse_has_payload(buffer: &[u8]) -> bool {
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

pub(super) fn next_sse_event_end(buffer: &[u8]) -> Option<usize> {
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

pub(super) fn stream_has_terminal_event(chunk: &[u8]) -> bool {
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

pub(super) fn stream_has_cancelled_event(chunk: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(chunk) else {
        return false;
    };
    if serde_json::from_str::<Value>(text.trim())
        .ok()
        .is_some_and(|value| is_cancelled_stream_value(&value))
    {
        return true;
    }
    text.lines().any(|line| {
        line.strip_prefix("data:")
            .map(str::trim)
            .and_then(|data| serde_json::from_str::<Value>(data).ok())
            .is_some_and(|value| is_cancelled_stream_value(&value))
    })
}

pub(super) fn is_terminal_stream_value(value: &Value) -> bool {
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

pub(super) fn is_cancelled_stream_value(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("response.cancelled" | "response.canceled")
    )
}

pub(super) fn stream_payload_error(chunk: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(chunk).ok()?;
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        if let Some(error) = stream_error_from_value(&value) {
            return Some(error);
        }
    }
    let mut error_event = false;
    for line in text.lines() {
        if line
            .strip_prefix("event:")
            .map(str::trim)
            .is_some_and(|event| matches!(event, "error" | "response.failed"))
        {
            error_event = true;
            continue;
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
    (error_event && (text.contains("\n\n") || text.contains("\r\n\r\n")))
        .then(|| "upstream emitted an error event".to_string())
}

pub(super) fn stream_error_from_value(value: &Value) -> Option<String> {
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
    let code = error
        .and_then(|error| error.get("code").and_then(Value::as_str))
        .or_else(|| value.get("code").and_then(Value::as_str));
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
    let summary = match code.filter(|code| !message.contains(code)) {
        Some(code) => format!("{code}: {message}"),
        None => message.to_string(),
    };
    Some(summary.chars().take(300).collect())
}

pub(super) fn is_transient_load_shed_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("server_is_overloaded") || message.contains("slow_down")
}

pub(super) fn completed_response_from_sse(text: &str) -> Option<Value> {
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

pub(super) fn filtered_response_headers(headers: &reqwest::header::HeaderMap) -> HeaderMap {
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

pub(super) fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let api_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok());
    bearer == Some(expected) || api_key == Some(expected)
}

pub(super) async fn upstream_error_summary(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| stream_error_from_value(&value))
        .map(|message| format!("{status} {message}"))
        .unwrap_or_else(|| status.to_string())
}

pub(super) fn passthrough_client_response(response: reqwest::Response) -> Response {
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
