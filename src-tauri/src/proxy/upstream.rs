use super::*;

pub(super) async fn ensure_account_ready(
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
    let client = state.client.load_full();
    let codex_version = crate::codex_identity::current_version(&state.codex_version);
    let refreshed = oauth::refresh_account(&client, &current, &codex_version).await?;
    state
        .db
        .update_oauth_tokens(&current.id, &refreshed)
        .map_err(|error| format!("保存 OAuth 刷新结果失败: {error}"))
}

pub(super) fn oauth_token_is_usable(account: &Account) -> bool {
    !account.access_token.is_empty()
        && account
            .expires_at
            .map(|expires| expires > chrono::Utc::now().timestamp() + 120)
            .unwrap_or(true)
}

pub(super) async fn send_upstream(
    client: &reqwest::Client,
    account: &Account,
    method: &Method,
    uri: &Uri,
    inbound_headers: &HeaderMap,
    body: &[u8],
    codex_version: &str,
) -> Result<reqwest::Response, SendUpstreamError> {
    let oauth_account = account.account_type == "oauth";
    let target = if oauth_account {
        oauth_target_url(uri, codex_version).map_err(SendUpstreamError::Request)?
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
        request = crate::codex_identity::apply_identity(request, codex_version)
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

pub(super) fn oauth_target_url(uri: &Uri, codex_version: &str) -> Result<String, String> {
    let path = uri.path();
    let mut target = if is_models_path(path) {
        CHATGPT_CODEX_MODELS_URL.to_string()
    } else if is_responses_path(path) {
        let suffix = safe_response_path_suffix_from(path)
            .ok_or_else(|| "Responses 子路径不符合安全规则".to_string())?;
        format!("{CHATGPT_CODEX_RESPONSES_URL}{suffix}")
    } else {
        return Err(format!("OAuth 账号不支持端点 {path}"));
    };
    if is_models_path(path) {
        target.push_str("?client_version=");
        target.push_str(codex_version);
    } else if let Some(query) = uri.query() {
        target.push('?');
        target.push_str(query);
    }
    Ok(target)
}

pub(super) fn api_key_target_url(account: &Account, uri: &Uri) -> Result<String, String> {
    let base = if account.base_url.trim().is_empty() {
        "https://api.openai.com"
    } else {
        account.base_url.trim()
    };
    let canonical_path = if is_models_path(uri.path()) {
        "/v1/models".to_string()
    } else if is_responses_path(uri.path()) {
        let suffix = safe_response_path_suffix_from(uri.path())
            .ok_or_else(|| "Responses 子路径不符合安全规则".to_string())?;
        format!("/v1/responses{suffix}")
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

pub(super) fn normalize_oauth_body(body: &[u8], compact: bool) -> Result<Vec<u8>, String> {
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

pub(super) fn include_chat_stream_usage(body: &[u8], path: &str) -> Vec<u8> {
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
