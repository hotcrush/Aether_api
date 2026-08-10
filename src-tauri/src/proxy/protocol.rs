use super::*;

pub(super) fn request_wants_stream(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        .unwrap_or(false)
}

pub(super) fn extract_model_hint(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("model").and_then(Value::as_str).map(String::from))
}

pub(super) fn request_uses_image_generation(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let tools_values = [value.get("tools"), value.pointer("/response/tools")];
    let uses_image_generation = tools_values.into_iter().flatten().any(|tools| {
        tools.as_array().is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("type").and_then(Value::as_str) == Some("image_generation"))
        })
    });
    uses_image_generation
}

pub(super) fn extract_usage_from_json_str(text: &str) -> Option<UsageBreakdown> {
    let value: Value = serde_json::from_str(text).ok()?;
    extract_usage_from_value(&value)
}

pub(super) fn extract_response_model_from_json_str(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    extract_response_model_from_value(&value)
}

pub(super) fn extract_response_model_from_sse(text: &str) -> Option<String> {
    let normalized = text.replace("\r\n", "\n");
    let mut first = None;
    let mut terminal = None;
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
        let Some(model) = extract_response_model_from_value(&value) else {
            continue;
        };
        let is_terminal = matches!(
            value.get("type").and_then(Value::as_str),
            Some(
                "response.completed"
                    | "response.done"
                    | "response.failed"
                    | "response.incomplete"
                    | "response.cancelled"
                    | "response.canceled"
            )
        );
        if is_terminal {
            terminal = Some(model);
        } else if first.is_none() {
            first = Some(model);
        }
    }
    terminal.or(first)
}

pub(super) fn extract_response_model_from_value(value: &Value) -> Option<String> {
    let response = value.get("response").unwrap_or(value);
    response
        .get("model")
        .or_else(|| value.get("model"))
        .or_else(|| response.get("modelVersion"))
        .or_else(|| value.get("modelVersion"))
        .or_else(|| value.pointer("/data/model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| model.chars().take(200).collect())
}

pub(super) fn extract_usage_from_sse(text: &str) -> Option<UsageBreakdown> {
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

pub(super) fn extract_usage_from_value(value: &Value) -> Option<UsageBreakdown> {
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

pub(super) fn extract_string_field_from_fragment(data: &[u8], field: &str) -> Option<String> {
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
        let Some(start) = remainder
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
        else {
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

pub(super) fn extract_usage_from_fragment(data: &[u8]) -> Option<UsageBreakdown> {
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
        let Some(start) = remainder
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
        else {
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

pub(super) fn usage_breakdown(
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

pub(super) fn token_value(value: Option<&Value>) -> i64 {
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

pub(super) fn balanced_json_object_end(value: &[u8]) -> Option<usize> {
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

pub(super) fn is_chat_completions_path(path: &str) -> bool {
    path.contains("/chat/completions")
}

pub(super) fn is_sse_response(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false)
}

pub(super) fn is_models_path(path: &str) -> bool {
    matches!(path, "/models" | "/v1/models" | "/backend-api/codex/models")
}

pub(super) fn is_responses_path(path: &str) -> bool {
    path == "/responses"
        || path.starts_with("/responses/")
        || path == "/v1/responses"
        || path.starts_with("/v1/responses/")
        || path == "/backend-api/codex/responses"
        || path.starts_with("/backend-api/codex/responses/")
}

const MAX_RESPONSE_PATH_SEGMENT_LENGTH: usize = 128;
const MAX_RESPONSE_PATH_SEGMENTS: usize = 8;

pub(super) fn is_forwardable_responses_path(path: &str) -> bool {
    !is_responses_path(path) || safe_response_path_suffix(response_path_suffix(path))
}

pub(super) fn safe_response_path_suffix_from(path: &str) -> Option<&str> {
    if !is_responses_path(path) {
        return None;
    }
    let suffix = response_path_suffix(path);
    safe_response_path_suffix(suffix).then_some(suffix)
}

fn safe_response_path_suffix(suffix: &str) -> bool {
    if suffix.is_empty() {
        return true;
    }
    let Some(raw_segments) = suffix.strip_prefix('/') else {
        return false;
    };
    let segments = raw_segments.split('/').collect::<Vec<_>>();
    if segments.len() > MAX_RESPONSE_PATH_SEGMENTS {
        return false;
    }
    segments.into_iter().all(safe_response_path_segment)
}

fn safe_response_path_segment(segment: &str) -> bool {
    if segment.is_empty() || segment.len() > MAX_RESPONSE_PATH_SEGMENT_LENGTH {
        return false;
    }
    let mut dots_only = true;
    for byte in segment.bytes() {
        let allowed = byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.');
        if !allowed {
            return false;
        }
        if byte != b'.' {
            dots_only = false;
        }
    }
    !dots_only
}

pub(super) fn is_compact_path(path: &str) -> bool {
    response_path_suffix(path).starts_with("/compact")
}

pub(super) fn response_path_suffix(path: &str) -> &str {
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

pub(super) fn json_response(status: StatusCode, value: Value) -> Response {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

pub(super) fn json_error(status: StatusCode, message: &str, error_type: &str) -> Response {
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
