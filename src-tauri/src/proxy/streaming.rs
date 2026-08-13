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
        if !image_generation::is_dedicated_account_id(&self.account_id) {
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
        if image_generation::is_dedicated_account_id(&self.account_id) {
            return;
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

    pub(super) fn record_upstream_model(&self, model: &str, terminal: bool) {
        if let Some(log) = &self.request_log {
            log.record_upstream_model(model, terminal);
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
    semantic_output_seen: bool,
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
            semantic_output_seen: false,
        }
    }

    pub(super) fn observe_chunk(&mut self, chunk: &[u8]) {
        self.capture_tail.extend_from_slice(chunk);
        if self.observed_model.is_none() {
            self.observed_model = extract_string_field_from_fragment(&self.capture_tail, "model");
            if let Some(model) = self.observed_model.as_deref() {
                self.context.record_upstream_model(model, false);
            }
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
        self.semantic_output_seen |= stream_has_semantic_output(event);
        let empty_completed = stream_has_empty_completed_event(event)
            && !self.semantic_output_seen
            && !self.usage_recorded;
        if let Ok(text) = std::str::from_utf8(event) {
            let model = if self.sse {
                extract_response_model_from_sse(text)
            } else {
                extract_response_model_from_json_str(text)
            };
            if let Some(model) = model {
                self.observed_model = Some(model.clone());
                self.context.record_upstream_model(&model, terminal);
            }
        }
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
        } else if empty_completed && !self.failure_recorded {
            self.failure_recorded = true;
            self.context.record_failure(
                CooldownScope::Capability,
                "upstream returned an empty response.completed event",
            );
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

    pub(super) fn record_client_cancelled(&mut self) {
        if self.failure_recorded {
            return;
        }
        self.failure_recorded = true;
        self.context.record_cancelled();
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
        if sse_bootstrap_is_empty_completed(text.as_bytes()) {
            return Err(PrepareResponseError::Upstream(
                "upstream returned an empty response.completed event".to_string(),
            ));
        }
        let usage = extract_usage_from_sse(&text);
        if let Some(model) = extract_response_model_from_sse(&text) {
            if let Some(context) = stream_observer.as_ref() {
                context.record_upstream_model(&model, true);
            }
        }
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
        if let Some(model) = std::str::from_utf8(&bytes)
            .ok()
            .and_then(extract_response_model_from_json_str)
        {
            if let Some(context) = stream_observer.as_ref() {
                context.record_upstream_model(&model, true);
            }
        }
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
        let mut sanitizer = ClientStreamSanitizer::new(sse);
        let first = sanitizer.push(&first);
        let client_stream = futures::stream::unfold(
            (stream, observer, sanitizer, first, false),
            |(mut stream, mut observer, mut sanitizer, mut pending, mut finished)| async move {
                loop {
                    if let Some(chunk) = pending.take() {
                        return Some((
                            Ok::<Bytes, std::io::Error>(chunk),
                            (stream, observer, sanitizer, pending, finished),
                        ));
                    }
                    if finished {
                        return None;
                    }
                    match stream.next().await {
                        Some(Ok(chunk)) => {
                            if let Some(observer) = observer.as_mut() {
                                observer.observe_chunk(&chunk);
                            }
                            pending = sanitizer.push(&chunk);
                        }
                        Some(Err(error)) => {
                            if let Some(observer) = observer.as_mut() {
                                observer.record_transport_failure(&error.to_string());
                            }
                            return Some((
                                Err(std::io::Error::other(error)),
                                (stream, observer, sanitizer, pending, true),
                            ));
                        }
                        None => {
                            if let Some(observer) = observer.as_mut() {
                                observer.record_eof();
                            }
                            finished = true;
                            pending = sanitizer.finish();
                        }
                    }
                }
            },
        );
        let mut builder = Response::builder().status(status);
        *builder.headers_mut().unwrap() = headers;
        return Ok((
            builder.body(Body::from_stream(client_stream)).unwrap(),
            None,
            true,
        ));
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
                if sse_bootstrap_is_empty_completed(&buffered) {
                    return Err(PrepareResponseError::Upstream(
                        "upstream returned an empty response.completed event".to_string(),
                    ));
                }
                if sse_has_semantic_payload(&buffered) {
                    return Ok(Bytes::from(buffered));
                }
                if buffered.len() > MAX_STREAM_BOOTSTRAP_BYTES {
                    return Err(PrepareResponseError::Upstream(
                        "upstream SSE bootstrap exceeded 8 MiB".to_string(),
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

fn sse_has_semantic_payload(buffer: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(buffer) else {
        return false;
    };
    let normalized = text.replace("\r\n", "\n");
    let Some(last_delimiter) = normalized.rfind("\n\n") else {
        return false;
    };
    normalized[..last_delimiter].lines().any(|line| {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            return false;
        };
        if data == "[DONE]" {
            return true;
        }
        serde_json::from_str::<Value>(data)
            .ok()
            .is_some_and(|value| {
                stream_value_has_semantic_output(&value)
                    || is_terminal_stream_value(&value)
                    || stream_payload_error(data.as_bytes()).is_some()
            })
    })
}

fn sse_bootstrap_is_empty_completed(buffer: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(buffer) else {
        return false;
    };
    let mut semantic_output = false;
    let mut usage = false;
    for line in text.replace("\r\n", "\n").lines() {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        semantic_output |= stream_value_has_semantic_output(&value);
        usage |= extract_usage_from_value(&value).is_some_and(|usage| usage.total_tokens > 0);
        if responses_completed_value_is_empty(&value) && !semantic_output && !usage {
            return true;
        }
    }
    false
}

pub(super) fn stream_has_semantic_output(chunk: &[u8]) -> bool {
    stream_values(chunk).any(|value| stream_value_has_semantic_output(&value))
}

pub(super) fn stream_has_empty_completed_event(chunk: &[u8]) -> bool {
    stream_values(chunk).any(|value| responses_completed_value_is_empty(&value))
}

pub(super) fn stream_has_usage(chunk: &[u8]) -> bool {
    stream_values(chunk)
        .any(|value| extract_usage_from_value(&value).is_some_and(|usage| usage.total_tokens > 0))
}

fn stream_values(chunk: &[u8]) -> impl Iterator<Item = Value> + '_ {
    std::str::from_utf8(chunk)
        .into_iter()
        .flat_map(|text| text.lines())
        .filter_map(|line| {
            let text = line
                .strip_prefix("data:")
                .map(str::trim)
                .unwrap_or(line.trim());
            serde_json::from_str::<Value>(text).ok()
        })
}

fn stream_value_has_semantic_output(value: &Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        None => true,
        Some(
            "response.created"
            | "response.in_progress"
            | "response.queued"
            | "response.completed"
            | "response.done"
            | "response.incomplete"
            | "response.cancelled"
            | "response.canceled"
            | "rate_limits.updated",
        ) => false,
        Some("error" | "response.failed") => false,
        Some(event_type) if event_type.ends_with(".delta") => {
            value.get("delta").is_some_and(|delta| match delta {
                Value::String(delta) => !delta.is_empty(),
                Value::Object(object) => !object.is_empty(),
                _ => false,
            })
        }
        Some(
            "response.output_text.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.done"
            | "response.audio_transcript.done",
        ) => value
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty()),
        Some("response.function_call_arguments.done") => value
            .get("arguments")
            .and_then(Value::as_str)
            .is_some_and(|arguments| !arguments.is_empty()),
        Some("response.custom_tool_call_input.done") => value
            .get("input")
            .and_then(Value::as_str)
            .is_some_and(|input| !input.is_empty()),
        Some("response.image_generation_call.partial_image") => value
            .get("partial_image_b64")
            .and_then(Value::as_str)
            .is_some_and(|image| !image.is_empty()),
        Some("response.content_part.added" | "response.content_part.done") => {
            value.get("part").is_some_and(value_has_visible_content)
        }
        Some("response.output_item.added" | "response.output_item.done") => {
            value.get("item").is_some_and(value_has_visible_content)
        }
        Some(_) => true,
    }
}

fn value_has_visible_content(value: &Value) -> bool {
    ["text", "transcript", "arguments", "input", "result"]
        .into_iter()
        .any(|field| {
            value
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|content| !content.is_empty())
        })
        || value
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(value_has_visible_content))
}

fn responses_completed_value_is_empty(value: &Value) -> bool {
    if !matches!(
        value.get("type").and_then(Value::as_str),
        Some("response.completed" | "response.done")
    ) {
        return false;
    }
    let response = value.get("response").unwrap_or(value);
    let has_usage = response.get("usage").is_some()
        || value.get("usage").is_some()
        || value.pointer("/data/usage").is_some()
        || value.pointer("/data/response/usage").is_some();
    let has_error = response.get("error").is_some() || value.get("error").is_some();
    let has_output = response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|output| !output.is_empty());
    !has_usage && !has_error && !has_output
}

struct ClientStreamSanitizer {
    sse: bool,
    buffer: Vec<u8>,
}

impl ClientStreamSanitizer {
    fn new(sse: bool) -> Self {
        Self {
            sse,
            buffer: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Option<Bytes> {
        if !self.sse {
            return (!chunk.is_empty()).then(|| Bytes::copy_from_slice(chunk));
        }
        self.buffer.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(event_end) = next_sse_event_end(&self.buffer) {
            let event = self.buffer.drain(..event_end).collect::<Vec<_>>();
            output.extend_from_slice(&sanitize_capacity_shed_sse_event(&event));
        }
        (!output.is_empty()).then(|| Bytes::from(output))
    }

    fn finish(&mut self) -> Option<Bytes> {
        if self.buffer.is_empty() {
            return None;
        }
        let remaining = std::mem::take(&mut self.buffer);
        Some(Bytes::from(sanitize_capacity_shed_sse_event(&remaining)))
    }
}

pub(super) fn sanitize_capacity_shed_sse_event(event: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(event) else {
        return event.to_vec();
    };
    let mut changed = false;
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (content, ending) = line
            .strip_suffix("\r\n")
            .map(|content| (content, "\r\n"))
            .or_else(|| line.strip_suffix('\n').map(|content| (content, "\n")))
            .unwrap_or((line, ""));
        let Some(data) = content.strip_prefix("data:") else {
            output.push_str(content);
            output.push_str(ending);
            continue;
        };
        let leading_space = data.starts_with(' ');
        let trimmed = data.trim();
        let Ok(mut value) = serde_json::from_str::<Value>(trimmed) else {
            output.push_str(content);
            output.push_str(ending);
            continue;
        };
        if !rewrite_capacity_shed_error_code(&mut value) {
            output.push_str(content);
            output.push_str(ending);
            continue;
        }
        changed = true;
        output.push_str("data:");
        if leading_space {
            output.push(' ');
        }
        output.push_str(&value.to_string());
        output.push_str(ending);
    }
    if changed {
        output.into_bytes()
    } else {
        event.to_vec()
    }
}

fn rewrite_capacity_shed_error_code(value: &mut Value) -> bool {
    let response_changed = value
        .pointer_mut("/response/error")
        .is_some_and(rewrite_capacity_shed_error_object);
    let error_changed = value
        .pointer_mut("/error")
        .is_some_and(rewrite_capacity_shed_error_object);
    response_changed || error_changed
}

fn rewrite_capacity_shed_error_object(error: &mut Value) -> bool {
    let Some(code) = error
        .as_object_mut()
        .and_then(|error| error.get_mut("code"))
    else {
        return false;
    };
    if !code
        .as_str()
        .is_some_and(|code| matches!(code, "server_is_overloaded" | "slow_down"))
    {
        return false;
    }
    *code = Value::String("server_error".to_string());
    true
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
    message.contains("server_is_overloaded")
        || message.contains("slow_down")
        || message.contains("no available account")
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
    upstream_error_summary_and_html(response).await.0
}

pub(super) async fn upstream_error_summary_and_html(response: reqwest::Response) -> (String, bool) {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let trimmed = body.trim_start();
    let lowercase = trimmed
        .chars()
        .take(32)
        .collect::<String>()
        .to_ascii_lowercase();
    let is_html = lowercase.starts_with("<!doctype html") || lowercase.starts_with("<html");
    let summary = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| stream_error_from_value(&value))
        .map(|message| format!("{status} {message}"))
        .unwrap_or_else(|| status.to_string());
    (summary, is_html)
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
