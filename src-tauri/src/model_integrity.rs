use crate::db::Account;
use crate::AppState;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_secs(35);
const MAX_ERROR_PREVIEW_CHARS: usize = 240;
const PROBE_COUNT: u8 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ModelIntegrityCheck {
    pub key: String,
    pub label: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ModelIntegrityResult {
    pub id: i64,
    pub account_id: String,
    pub requested_model: String,
    pub declared: Option<bool>,
    pub observed_models: Vec<String>,
    pub risk: String,
    pub score: u8,
    pub summary: String,
    pub checks: Vec<ModelIntegrityCheck>,
    pub probe_count: u8,
    pub successful_probes: u8,
    pub total_tokens: i64,
    pub reasoning_tokens: i64,
    pub duration_ms: i64,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeEndpoint {
    ChatCompletions,
    Responses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatTokenParameter {
    MaxTokens,
    MaxCompletionTokens,
}

#[derive(Clone, Copy, Debug)]
enum ProbeKind {
    StructuredOutput,
    ToolCall,
    ContextRecall,
}

impl ProbeKind {
    fn key(self) -> &'static str {
        match self {
            Self::StructuredOutput => "structured_output",
            Self::ToolCall => "tool_call",
            Self::ContextRecall => "context_recall",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::StructuredOutput => "结构化输出",
            Self::ToolCall => "工具调用",
            Self::ContextRecall => "多轮指令保持",
        }
    }
}

#[derive(Debug)]
struct Challenge {
    nonce: String,
    left: i64,
    right: i64,
    offset: i64,
}

impl Challenge {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4();
        let bytes = id.as_bytes();
        Self {
            nonce: id.simple().to_string()[..12].to_string(),
            left: i64::from(bytes[0] % 71 + 19),
            right: i64::from(bytes[1] % 61 + 17),
            offset: i64::from(bytes[2] % 31 + 7),
        }
    }

    fn answer(&self) -> i64 {
        self.left * 3 + self.right * 2 - self.offset
    }
}

#[derive(Debug)]
struct RawProbe {
    status: Option<StatusCode>,
    value: Option<Value>,
    error: String,
    endpoint: ProbeEndpoint,
}

impl RawProbe {
    fn succeeded(&self) -> bool {
        self.status.is_some_and(|status| status.is_success()) && self.value.is_some()
    }
}

#[derive(Debug)]
struct ProbeEvaluation {
    request_succeeded: bool,
    capability_passed: bool,
    observed_model: Option<String>,
    model_mismatch: bool,
    total_tokens: i64,
    reasoning_tokens: i64,
    checks: Vec<ModelIntegrityCheck>,
}

#[tauri::command]
pub(crate) async fn probe_model_integrity(
    state: tauri::State<'_, AppState>,
    account_id: String,
    model: String,
) -> Result<ModelIntegrityResult, String> {
    let account_id = account_id.trim();
    let model = validate_model(&model)?;
    let account = state
        .db
        .get_account(account_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "渠道不存在".to_string())?;
    validate_account(&account)?;

    let started_at = Instant::now();
    let client = state.client.load_full();
    let (declared, mut checks) = inspect_model_declaration(&client, &account, &model).await;
    let challenge = Challenge::new();
    let mut preferred_endpoint = ProbeEndpoint::ChatCompletions;
    let mut evaluations = Vec::with_capacity(PROBE_COUNT as usize);

    for kind in [
        ProbeKind::StructuredOutput,
        ProbeKind::ToolCall,
        ProbeKind::ContextRecall,
    ] {
        let mut raw = execute_probe(
            &client,
            &account,
            &model,
            kind,
            &challenge,
            preferred_endpoint,
        )
        .await;
        if preferred_endpoint == ProbeEndpoint::ChatCompletions
            && should_try_responses_endpoint(&raw)
        {
            let fallback = execute_probe(
                &client,
                &account,
                &model,
                kind,
                &challenge,
                ProbeEndpoint::Responses,
            )
            .await;
            if fallback.succeeded() || !raw.succeeded() {
                raw = fallback;
            }
        }
        if raw.succeeded() {
            preferred_endpoint = raw.endpoint;
        }
        evaluations.push(evaluate_probe(raw, kind, &challenge, &model));
    }

    for evaluation in &evaluations {
        checks.extend(evaluation.checks.clone());
    }
    let observed_models = evaluations
        .iter()
        .filter_map(|evaluation| evaluation.observed_model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let successful_probes = evaluations
        .iter()
        .filter(|evaluation| evaluation.request_succeeded)
        .count() as u8;
    let total_tokens = evaluations
        .iter()
        .map(|evaluation| evaluation.total_tokens)
        .sum::<i64>();
    let reasoning_tokens = evaluations
        .iter()
        .map(|evaluation| evaluation.reasoning_tokens)
        .sum::<i64>();
    let (score, risk, summary) = score_result(declared, &evaluations);
    let result = ModelIntegrityResult {
        id: 0,
        account_id: account.id,
        requested_model: model,
        declared,
        observed_models,
        risk,
        score,
        summary,
        checks,
        probe_count: PROBE_COUNT,
        successful_probes,
        total_tokens,
        reasoning_tokens,
        duration_ms: elapsed_millis(started_at),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    state
        .db
        .insert_model_integrity_result(&result)
        .map_err(|error| format!("保存验模结果失败: {error}"))
}

#[tauri::command]
pub(crate) fn list_model_integrity_history(
    state: tauri::State<'_, AppState>,
    account_id: String,
    limit: Option<i64>,
) -> Result<Vec<ModelIntegrityResult>, String> {
    state
        .db
        .list_model_integrity_results(account_id.trim(), limit.unwrap_or(10))
        .map_err(|error| error.to_string())
}

fn validate_model(model: &str) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("请输入要检测的模型".to_string());
    }
    if model.chars().count() > 200 || model.chars().any(char::is_control) {
        return Err("模型名称无效".to_string());
    }
    Ok(model.to_string())
}

fn validate_account(account: &Account) -> Result<(), String> {
    if account.account_type != "api_key" {
        return Err("模型真实性检测用于 API Key 中转站；OAuth 账号可作为可信基准源".to_string());
    }
    if account.status != "active" {
        return Err("渠道已停用".to_string());
    }
    if account.api_key.trim().is_empty() {
        return Err("渠道未配置 API Key".to_string());
    }
    endpoint_url(account, "/v1/models").map(|_| ())
}

async fn inspect_model_declaration(
    client: &reqwest::Client,
    account: &Account,
    requested_model: &str,
) -> (Option<bool>, Vec<ModelIntegrityCheck>) {
    let Ok(url) = endpoint_url(account, "/v1/models") else {
        return (None, Vec::new());
    };
    let response = tokio::time::timeout(
        PROBE_TIMEOUT,
        client.get(url).bearer_auth(account.api_key.trim()).send(),
    )
    .await;
    let response = match response {
        Err(_) => {
            return (
                None,
                vec![check(
                    "model_declaration",
                    "模型声明",
                    "warn",
                    "模型列表请求超时，继续执行主动探针",
                )],
            )
        }
        Ok(Err(error)) => {
            return (
                None,
                vec![check(
                    "model_declaration",
                    "模型声明",
                    "warn",
                    format!("无法读取模型列表: {}", transport_error(&error)),
                )],
            )
        }
        Ok(Ok(response)) => response,
    };
    let status = response.status();
    if !status.is_success() {
        return (
            None,
            vec![check(
                "model_declaration",
                "模型声明",
                "warn",
                format!("模型列表返回 HTTP {status}，继续执行主动探针"),
            )],
        );
    }
    let value = match tokio::time::timeout(PROBE_TIMEOUT, response.json::<Value>()).await {
        Ok(Ok(value)) => value,
        _ => {
            return (
                None,
                vec![check(
                    "model_declaration",
                    "模型声明",
                    "warn",
                    "模型列表不是可识别的 JSON",
                )],
            )
        }
    };
    let models = extract_declared_models(&value);
    if models.is_empty() {
        return (
            None,
            vec![check(
                "model_declaration",
                "模型声明",
                "warn",
                "模型列表中没有可识别的模型 ID",
            )],
        );
    }
    let declared = models
        .iter()
        .any(|observed| models_compatible(requested_model, observed));
    let message = if declared {
        format!("模型列表已声明 {requested_model}")
    } else {
        format!(
            "模型列表未声明 {requested_model}（共返回 {} 个模型）",
            models.len()
        )
    };
    (
        Some(declared),
        vec![check(
            "model_declaration",
            "模型声明",
            if declared { "pass" } else { "fail" },
            message,
        )],
    )
}

async fn execute_probe(
    client: &reqwest::Client,
    account: &Account,
    model: &str,
    kind: ProbeKind,
    challenge: &Challenge,
    endpoint: ProbeEndpoint,
) -> RawProbe {
    let mut token_parameter = ChatTokenParameter::MaxTokens;
    let mut raw = send_probe(
        client,
        account,
        model,
        kind,
        challenge,
        endpoint,
        token_parameter,
    )
    .await;
    if endpoint == ProbeEndpoint::ChatCompletions && rejects_max_tokens(&raw) {
        token_parameter = ChatTokenParameter::MaxCompletionTokens;
        raw = send_probe(
            client,
            account,
            model,
            kind,
            challenge,
            endpoint,
            token_parameter,
        )
        .await;
    }
    raw
}

async fn send_probe(
    client: &reqwest::Client,
    account: &Account,
    model: &str,
    kind: ProbeKind,
    challenge: &Challenge,
    endpoint: ProbeEndpoint,
    token_parameter: ChatTokenParameter,
) -> RawProbe {
    let path = match endpoint {
        ProbeEndpoint::ChatCompletions => "/v1/chat/completions",
        ProbeEndpoint::Responses => "/v1/responses",
    };
    let url = match endpoint_url(account, path) {
        Ok(url) => url,
        Err(error) => {
            return RawProbe {
                status: None,
                value: None,
                error,
                endpoint,
            }
        }
    };
    let body = probe_body(model, kind, challenge, endpoint, token_parameter);
    let response = tokio::time::timeout(
        PROBE_TIMEOUT,
        client
            .post(url)
            .bearer_auth(account.api_key.trim())
            .header("Accept", "application/json")
            .json(&body)
            .send(),
    )
    .await;
    let response = match response {
        Err(_) => {
            return RawProbe {
                status: None,
                value: None,
                error: "请求超时".to_string(),
                endpoint,
            }
        }
        Ok(Err(error)) => {
            return RawProbe {
                status: None,
                value: None,
                error: transport_error(&error),
                endpoint,
            }
        }
        Ok(Ok(response)) => response,
    };
    let status = response.status();
    let body = match tokio::time::timeout(PROBE_TIMEOUT, response.text()).await {
        Ok(Ok(body)) => body,
        Ok(Err(error)) => transport_error(&error),
        Err(_) => "读取响应超时".to_string(),
    };
    let value = serde_json::from_str::<Value>(&body).ok();
    let error = if status.is_success() && value.is_some() {
        String::new()
    } else if status.is_success() {
        "响应不是有效 JSON".to_string()
    } else {
        let summary = api_error_message(value.as_ref())
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| body.chars().take(MAX_ERROR_PREVIEW_CHARS).collect());
        format!("HTTP {status}: {summary}")
    };
    RawProbe {
        status: Some(status),
        value,
        error,
        endpoint,
    }
}

fn probe_body(
    model: &str,
    kind: ProbeKind,
    challenge: &Challenge,
    endpoint: ProbeEndpoint,
    token_parameter: ChatTokenParameter,
) -> Value {
    let system = "You are an API capability verifier. Follow the requested protocol exactly. Do not explain your answer.";
    let answer = challenge.answer();
    let messages = match kind {
        ProbeKind::StructuredOutput => vec![
            json!({"role": "system", "content": system}),
            json!({
                "role": "user",
                "content": format!(
                    "Return only a JSON object with exactly two fields: nonce must be \"{}\" and answer must be the integer result of ({} * 3) + ({} * 2) - {}.",
                    challenge.nonce, challenge.left, challenge.right, challenge.offset
                )
            }),
        ],
        ProbeKind::ToolCall => vec![
            json!({"role": "system", "content": system}),
            json!({
                "role": "user",
                "content": format!(
                    "Call record_probe exactly once with nonce \"{}\" and answer {}. Do not emit a normal text response.",
                    challenge.nonce, answer
                )
            }),
        ],
        ProbeKind::ContextRecall => vec![
            json!({"role": "system", "content": system}),
            json!({
                "role": "user",
                "content": format!(
                    "Remember nonce \"{}\", left {}, right {}, and offset {}. Reply only ACK.",
                    challenge.nonce, challenge.left, challenge.right, challenge.offset
                )
            }),
            json!({"role": "assistant", "content": "ACK"}),
            json!({
                "role": "user",
                "content": "Using only the values from the earlier message, return JSON with nonce and answer=(left*3)+(right*2)-offset. No markdown."
            }),
        ],
    };

    let mut body = match endpoint {
        ProbeEndpoint::ChatCompletions => json!({
            "model": model,
            "messages": messages,
            "stream": false,
        }),
        ProbeEndpoint::Responses => json!({
            "model": model,
            "input": messages,
            "stream": false,
            "store": false,
            "max_output_tokens": 192,
        }),
    };

    if endpoint == ProbeEndpoint::ChatCompletions {
        let key = match token_parameter {
            ChatTokenParameter::MaxTokens => "max_tokens",
            ChatTokenParameter::MaxCompletionTokens => "max_completion_tokens",
        };
        body.as_object_mut()
            .expect("probe request is an object")
            .insert(key.to_string(), Value::from(192));
    }

    match kind {
        ProbeKind::StructuredOutput if endpoint == ProbeEndpoint::ChatCompletions => {
            body.as_object_mut()
                .expect("probe request is an object")
                .insert(
                    "response_format".to_string(),
                    json!({"type": "json_object"}),
                );
        }
        ProbeKind::ToolCall => {
            let parameters = json!({
                "type": "object",
                "properties": {
                    "nonce": {"type": "string"},
                    "answer": {"type": "integer"}
                },
                "required": ["nonce", "answer"],
                "additionalProperties": false
            });
            let (tools, tool_choice) = match endpoint {
                ProbeEndpoint::ChatCompletions => (
                    json!([{
                        "type": "function",
                        "function": {
                            "name": "record_probe",
                            "description": "Record a model integrity probe result.",
                            "parameters": parameters,
                            "strict": true
                        }
                    }]),
                    json!({"type": "function", "function": {"name": "record_probe"}}),
                ),
                ProbeEndpoint::Responses => (
                    json!([{
                        "type": "function",
                        "name": "record_probe",
                        "description": "Record a model integrity probe result.",
                        "parameters": parameters,
                        "strict": true
                    }]),
                    json!({"type": "function", "name": "record_probe"}),
                ),
            };
            let object = body.as_object_mut().expect("probe request is an object");
            object.insert("tools".to_string(), tools);
            object.insert("tool_choice".to_string(), tool_choice);
        }
        _ => {}
    }
    body
}

fn evaluate_probe(
    raw: RawProbe,
    kind: ProbeKind,
    challenge: &Challenge,
    requested_model: &str,
) -> ProbeEvaluation {
    let mut checks = Vec::new();
    if !raw.succeeded() {
        checks.push(check(
            format!("probe_{}", kind.key()),
            kind.label(),
            "fail",
            if raw.error.is_empty() {
                "探针请求失败".to_string()
            } else {
                raw.error.clone()
            },
        ));
        return ProbeEvaluation {
            request_succeeded: false,
            capability_passed: false,
            observed_model: None,
            model_mismatch: false,
            total_tokens: 0,
            reasoning_tokens: 0,
            checks,
        };
    }

    let value = raw.value.as_ref().expect("successful probe has JSON");
    let observed_model = extract_observed_model(value);
    let model_mismatch = observed_model
        .as_deref()
        .is_some_and(|observed| !models_compatible(requested_model, observed));
    checks.push(match observed_model.as_deref() {
        Some(observed) if model_mismatch => check(
            format!("model_{}", kind.key()),
            "响应模型",
            "fail",
            format!("请求 {requested_model}，响应声明为 {observed}"),
        ),
        Some(observed) => check(
            format!("model_{}", kind.key()),
            "响应模型",
            "pass",
            format!("响应声明为 {observed}"),
        ),
        None => check(
            format!("model_{}", kind.key()),
            "响应模型",
            "warn",
            "响应未提供 model/modelVersion，无法核对元数据",
        ),
    });

    let capability_passed = match kind {
        ProbeKind::ToolCall => extract_tool_arguments(value)
            .and_then(|arguments| parse_json_payload(&arguments))
            .is_some_and(|payload| payload_matches(&payload, challenge)),
        ProbeKind::StructuredOutput | ProbeKind::ContextRecall => extract_output_text(value)
            .and_then(|content| parse_json_payload(&content))
            .is_some_and(|payload| payload_matches(&payload, challenge)),
    };
    let endpoint_label = match raw.endpoint {
        ProbeEndpoint::ChatCompletions => "Chat Completions",
        ProbeEndpoint::Responses => "Responses",
    };
    checks.push(check(
        format!("capability_{}", kind.key()),
        kind.label(),
        if capability_passed { "pass" } else { "fail" },
        if capability_passed {
            format!("{endpoint_label} 探针结果正确")
        } else {
            format!("{endpoint_label} 返回内容未通过动态挑战校验")
        },
    ));

    let (total_tokens, reasoning_tokens) = extract_usage(value);
    ProbeEvaluation {
        request_succeeded: true,
        capability_passed,
        observed_model,
        model_mismatch,
        total_tokens,
        reasoning_tokens,
        checks,
    }
}

fn score_result(declared: Option<bool>, evaluations: &[ProbeEvaluation]) -> (u8, String, String) {
    let successful = evaluations
        .iter()
        .filter(|evaluation| evaluation.request_succeeded)
        .count();
    let mismatches = evaluations
        .iter()
        .filter(|evaluation| evaluation.model_mismatch)
        .count();
    let capability_failures = evaluations
        .iter()
        .filter(|evaluation| evaluation.request_succeeded && !evaluation.capability_passed)
        .count();
    let missing_model = evaluations
        .iter()
        .filter(|evaluation| evaluation.request_succeeded && evaluation.observed_model.is_none())
        .count();
    let request_failures = evaluations.len().saturating_sub(successful);

    let mut penalty = 0_i32;
    if declared == Some(false) {
        penalty += 18;
    }
    penalty += (mismatches as i32 * 30).min(50);
    penalty += capability_failures as i32 * 15;
    penalty += request_failures as i32 * 12;
    penalty += (missing_model as i32 * 3).min(9);
    let score = (100 - penalty).clamp(0, 100) as u8;

    if mismatches >= 2 || (successful >= 2 && score < 50) {
        return (
            score,
            "high_risk".to_string(),
            "多项证据与标称模型不一致，存在明显降级或错误路由风险".to_string(),
        );
    }
    if successful < 2 {
        return (
            score,
            "inconclusive".to_string(),
            "有效探针不足，当前无法判断模型真实性".to_string(),
        );
    }
    if mismatches > 0 || declared == Some(false) || capability_failures > 0 || score < 80 {
        return (
            score,
            "suspicious".to_string(),
            "部分元数据或能力指纹异常，建议复测并与官方源对照".to_string(),
        );
    }
    (
        score,
        "normal".to_string(),
        "三组主动探针与标称模型一致，暂未发现明显掺水信号".to_string(),
    )
}

fn endpoint_url(account: &Account, canonical_path: &str) -> Result<reqwest::Url, String> {
    let base = if account.base_url.trim().is_empty() {
        "https://api.openai.com"
    } else {
        account.base_url.trim()
    };
    let target =
        if base.trim_end_matches('/').ends_with("/v1") && canonical_path.starts_with("/v1/") {
            format!(
                "{}{}",
                base.trim_end_matches('/'),
                canonical_path.trim_start_matches("/v1")
            )
        } else {
            format!("{}{}", base.trim_end_matches('/'), canonical_path)
        };
    reqwest::Url::parse(&target).map_err(|error| format!("Base URL 无效: {error}"))
}

fn should_try_responses_endpoint(raw: &RawProbe) -> bool {
    if raw.succeeded() {
        return false;
    }
    raw.status.is_none_or(|status| {
        matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        )
    })
}

fn rejects_max_tokens(raw: &RawProbe) -> bool {
    raw.status == Some(StatusCode::BAD_REQUEST)
        && raw.error.to_ascii_lowercase().contains("max_tokens")
}

fn extract_declared_models(value: &Value) -> Vec<String> {
    let candidates = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.get("models").and_then(Value::as_array))
        .or_else(|| value.as_array());
    let mut models = BTreeSet::new();
    for candidate in candidates.into_iter().flatten() {
        let model = candidate.as_str().or_else(|| {
            candidate
                .get("id")
                .or_else(|| candidate.get("model"))
                .and_then(Value::as_str)
        });
        if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
            models.insert(model.to_string());
        }
    }
    models.into_iter().collect()
}

fn extract_observed_model(value: &Value) -> Option<String> {
    value
        .get("model")
        .or_else(|| value.get("modelVersion"))
        .or_else(|| value.pointer("/response/model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

fn extract_output_text(value: &Value) -> Option<String> {
    if let Some(content) = value.pointer("/choices/0/message/content") {
        if let Some(text) = content.as_str() {
            return Some(text.to_string());
        }
        if let Some(parts) = content.as_array() {
            if let Some(text) = parts.iter().find_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.get("content").and_then(Value::as_str))
            }) {
                return Some(text.to_string());
            }
        }
    }
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .find_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.get("output_text").and_then(Value::as_str))
        })
        .map(str::to_string)
}

fn extract_tool_arguments(value: &Value) -> Option<String> {
    if let Some(arguments) = value
        .pointer("/choices/0/message/tool_calls/0/function/arguments")
        .and_then(Value::as_str)
    {
        return Some(arguments.to_string());
    }
    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("name").and_then(Value::as_str) == Some("record_probe")
        })
        .and_then(|item| item.get("arguments"))
        .and_then(|arguments| {
            arguments
                .as_str()
                .map(str::to_string)
                .or_else(|| serde_json::to_string(arguments).ok())
        })
}

fn extract_usage(value: &Value) -> (i64, i64) {
    let usage = value
        .get("usage")
        .or_else(|| value.pointer("/response/usage"));
    let Some(usage) = usage else {
        return (0, 0);
    };
    let input = integer_value(
        usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens")),
    );
    let output = integer_value(
        usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens")),
    );
    let total = integer_value(usage.get("total_tokens")).max(input.saturating_add(output));
    let reasoning = integer_value(
        usage
            .pointer("/output_tokens_details/reasoning_tokens")
            .or_else(|| usage.pointer("/completion_tokens_details/reasoning_tokens"))
            .or_else(|| usage.get("reasoning_tokens")),
    );
    (total.max(0), reasoning.max(0))
}

fn integer_value(value: Option<&Value>) -> i64 {
    value
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or(0)
}

fn parse_json_payload(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end >= start)
        .then(|| &text[start..=end])
        .and_then(|candidate| serde_json::from_str(candidate).ok())
}

fn payload_matches(payload: &Value, challenge: &Challenge) -> bool {
    payload.get("nonce").and_then(Value::as_str) == Some(challenge.nonce.as_str())
        && integer_value(payload.get("answer")) == challenge.answer()
}

fn models_compatible(requested: &str, observed: &str) -> bool {
    let requested = normalize_model_name(requested);
    let observed = normalize_model_name(observed);
    if requested == observed {
        return true;
    }
    model_has_version_suffix(&observed, &requested)
        || model_has_version_suffix(&requested, &observed)
}

fn normalize_model_name(model: &str) -> String {
    let model = model.trim().to_ascii_lowercase();
    ["openai/", "azure/", "models/"]
        .iter()
        .find_map(|prefix| model.strip_prefix(prefix))
        .unwrap_or(&model)
        .to_string()
}

fn model_has_version_suffix(candidate: &str, base: &str) -> bool {
    let Some(suffix) = candidate
        .strip_prefix(base)
        .and_then(|value| value.strip_prefix('-'))
    else {
        return false;
    };
    suffix == "latest"
        || suffix == "preview"
        || (!suffix.is_empty()
            && suffix
                .chars()
                .all(|character| character.is_ascii_digit() || character == '-'))
}

fn api_error_message(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
        })
        .and_then(Value::as_str)
        .map(|message| message.chars().take(MAX_ERROR_PREVIEW_CHARS).collect())
}

fn transport_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "请求超时".to_string()
    } else if error.is_connect() {
        "无法连接中转站".to_string()
    } else {
        error
            .to_string()
            .chars()
            .take(MAX_ERROR_PREVIEW_CHARS)
            .collect()
    }
}

fn check(
    key: impl Into<String>,
    label: impl Into<String>,
    status: impl Into<String>,
    message: impl Into<String>,
) -> ModelIntegrityCheck {
    ModelIntegrityCheck {
        key: key.into(),
        label: label.into(),
        status: status.into(),
        message: message.into(),
    }
}

fn elapsed_millis(started_at: Instant) -> i64 {
    started_at.elapsed().as_millis().min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_aliases_only_accept_version_suffixes() {
        assert!(models_compatible("gpt-5.4", "openai/gpt-5.4-2026-03-05"));
        assert!(models_compatible("gpt-5.4-latest", "gpt-5.4"));
        assert!(!models_compatible("gpt-5", "gpt-5-mini"));
        assert!(!models_compatible("gpt-5.4", "gpt-4.1"));
    }

    #[test]
    fn declared_models_support_common_response_shapes() {
        let openai = json!({"data": [{"id": "gpt-5"}, {"id": "gpt-5-mini"}]});
        assert_eq!(
            extract_declared_models(&openai),
            vec!["gpt-5", "gpt-5-mini"]
        );
        let relay = json!({"models": ["gpt-5", {"model": "gpt-4.1"}]});
        assert_eq!(extract_declared_models(&relay), vec!["gpt-4.1", "gpt-5"]);
    }

    #[test]
    fn parses_chat_and_responses_outputs() {
        let challenge = Challenge {
            nonce: "probe123".to_string(),
            left: 20,
            right: 30,
            offset: 7,
        };
        let payload = json!({"nonce": challenge.nonce, "answer": challenge.answer()});
        let chat = json!({
            "choices": [{"message": {"content": payload.to_string()}}]
        });
        assert!(payload_matches(
            &parse_json_payload(&extract_output_text(&chat).unwrap()).unwrap(),
            &challenge
        ));
        let responses = json!({
            "output": [{"type": "message", "content": [{"type": "output_text", "text": payload.to_string()}]}]
        });
        assert!(payload_matches(
            &parse_json_payload(&extract_output_text(&responses).unwrap()).unwrap(),
            &challenge
        ));
    }
}
