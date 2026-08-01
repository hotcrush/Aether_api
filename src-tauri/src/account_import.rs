use crate::db::{normalize_models, NewAccount};
use crate::oauth::{decode_token_metadata, merge_metadata, OPENAI_CLIENT_ID};
use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardImportSource {
    Cpa,
    Sub2api,
}

#[derive(Debug, Clone)]
pub struct ParsedClipboardImport {
    pub source: ClipboardImportSource,
    pub accounts: Vec<NewAccount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportMessage {
    pub index: usize,
    pub name: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ImportResult {
    pub total: usize,
    pub created: usize,
    pub updated: usize,
    pub failed: usize,
    pub errors: Vec<ImportMessage>,
}

pub fn parse_import_contents(contents: &[String]) -> (Vec<NewAccount>, Vec<ImportMessage>) {
    let mut accounts = Vec::new();
    let mut errors = Vec::new();
    let mut index = 0;

    for content in contents {
        if content.len() > MAX_IMPORT_BYTES {
            errors.push(ImportMessage {
                index: index + 1,
                name: String::new(),
                message: "单个导入内容不能超过 10 MB".to_string(),
            });
            index += 1;
            continue;
        }
        match parse_values(content) {
            Ok(values) => {
                for value in values {
                    index += 1;
                    match account_from_value(value) {
                        Ok(account) => accounts.push(account),
                        Err(message) => errors.push(ImportMessage {
                            index,
                            name: String::new(),
                            message,
                        }),
                    }
                }
            }
            Err(message) => {
                index += 1;
                errors.push(ImportMessage {
                    index,
                    name: String::new(),
                    message,
                });
            }
        }
    }
    (accounts, errors)
}

pub fn parse_clipboard_import(content: &str) -> Result<ParsedClipboardImport, String> {
    if content.len() > MAX_IMPORT_BYTES {
        return Err("剪贴板导入内容不能超过 10 MB".to_string());
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("剪贴板内容为空".to_string());
    }

    // Fast path: entire content is valid JSON.
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let source = classify_clipboard_value(&value)?;
        let (accounts, errors) = parse_import_contents(&[trimmed.to_string()]);
        if !errors.is_empty() || accounts.is_empty() {
            let message = errors
                .first()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| "没有找到可导入的账号".to_string());
            return Err(message);
        }
        return Ok(ParsedClipboardImport { source, accounts });
    }

    // Fallback: extract JSON values from mixed/noisy text.
    let extracted = extract_json_values(trimmed);
    if extracted.is_empty() {
        return Err("剪贴板内容不是完整 JSON，也未找到可识别的 JSON 片段".to_string());
    }
    let source = classify_extracted_values(&extracted)?;
    // Flatten and convert extracted values into accounts via the standard pipeline.
    let mut flat_values = Vec::new();
    for value in extracted {
        flatten_value(value, &mut flat_values);
    }
    let mut accounts = Vec::new();
    let mut errors = Vec::new();
    for (index, value) in flat_values.into_iter().enumerate() {
        match account_from_value(value) {
            Ok(account) => accounts.push(account),
            Err(message) => errors.push(ImportMessage {
                index: index + 1,
                name: String::new(),
                message,
            }),
        }
    }
    if accounts.is_empty() {
        let message = errors
            .first()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "没有找到可导入的账号".to_string());
        return Err(message);
    }
    Ok(ParsedClipboardImport { source, accounts })
}

fn classify_clipboard_value(value: &Value) -> Result<ClipboardImportSource, String> {
    if is_cpa_account(value) {
        return Ok(ClipboardImportSource::Cpa);
    }
    if let Some(values) = value.as_array() {
        if !values.is_empty() && values.iter().all(is_cpa_account) {
            return Ok(ClipboardImportSource::Cpa);
        }
    }
    if is_sub2api_account(value) || is_sub2api_backup(value) {
        return Ok(ClipboardImportSource::Sub2api);
    }
    Err("剪贴板内容不是受支持的 CPA 或 Sub2API JSON".to_string())
}

/// Classify multiple extracted JSON values, ensuring they are all the same source type.
fn classify_extracted_values(values: &[Value]) -> Result<ClipboardImportSource, String> {
    let mut source: Option<ClipboardImportSource> = None;
    for value in values {
        let detected = if is_cpa_account(value) {
            ClipboardImportSource::Cpa
        } else if is_sub2api_account(value) || is_sub2api_backup(value) {
            ClipboardImportSource::Sub2api
        } else if let Some(items) = value.as_array() {
            // Array: classify by its elements.
            if !items.is_empty() && items.iter().all(is_cpa_account) {
                ClipboardImportSource::Cpa
            } else if !items.is_empty() && items.iter().all(|v| is_sub2api_account(v) || is_sub2api_backup(v)) {
                ClipboardImportSource::Sub2api
            } else {
                continue; // unrecognizable array – skip
            }
        } else {
            continue; // unrecognizable object – skip
        };
        match source {
            None => source = Some(detected),
            Some(existing) if existing != detected => {
                return Err("剪贴板中混合了 CPA 和 Sub2API 格式，请分开导入".to_string());
            }
            _ => {}
        }
    }
    source.ok_or_else(|| "剪贴板内容不是受支持的 CPA 或 Sub2API JSON".to_string())
}

/// Scan mixed text and extract all complete top-level JSON values (objects or arrays).
/// Noise lines (non-JSON text) are silently discarded.
fn extract_json_values(text: &str) -> Vec<Value> {
    let mut values = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        let ch = bytes[i];
        if ch == b'{' || ch == b'[' {
            // Try to find the matching close bracket.
            if let Some(end) = find_json_end(bytes, i) {
                let candidate = &text[i..end];
                if let Ok(value) = serde_json::from_str::<Value>(candidate) {
                    values.push(value);
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    values
}

/// Starting at `start` (which must be `{` or `[`), find the byte offset just past
/// the matching close bracket, respecting strings and escapes.
fn find_json_end(bytes: &[u8], start: usize) -> Option<usize> {
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let mut i = start;
    while i < bytes.len() {
        let ch = bytes[i];
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }
        if in_string {
            match ch {
                b'\\' => escape_next = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match ch {
                b'"' => in_string = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 && ch == close {
                        return Some(i + 1);
                    }
                    if depth < 0 {
                        return None;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn is_cpa_account(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    string_field_eq(object.get("type"), "codex")
        && !object.contains_key("credentials")
        && has_non_empty_string(object.get("access_token"), object.get("refresh_token"))
}

fn is_sub2api_account(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let account_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    string_field_eq(object.get("platform"), "openai")
        && object
            .get("credentials")
            .and_then(Value::as_object)
            .is_some()
        && matches!(
            account_type.to_ascii_lowercase().as_str(),
            "oauth" | "apikey" | "api_key" | "key"
        )
}

fn is_sub2api_backup(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if !string_field_eq(object.get("type"), "sub2api-data") {
        return false;
    }
    let accounts = object
        .get("accounts")
        .or_else(|| object.get("data").and_then(|data| data.get("accounts")))
        .and_then(Value::as_array);
    matches!(accounts, Some(accounts) if !accounts.is_empty() && accounts.iter().all(is_sub2api_account))
}

fn string_field_eq(value: Option<&Value>, expected: &str) -> bool {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn has_non_empty_string(first: Option<&Value>, second: Option<&Value>) -> bool {
    [first, second].into_iter().flatten().any(|value| {
        value
            .as_str()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    })
}

fn parse_values(content: &str) -> Result<Vec<Value>, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("导入内容为空".to_string());
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let mut values = Vec::new();
        flatten_value(value, &mut values);
        return Ok(values);
    }

    let mut values = Vec::new();
    for (line_index, line) in trimmed.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if (line.starts_with('{') || line.starts_with('['))
            && serde_json::from_str::<Value>(line).is_err()
        {
            return Err(format!("第 {} 行 JSON 格式无效", line_index + 1));
        }
        let value =
            serde_json::from_str::<Value>(line).unwrap_or_else(|_| Value::String(line.to_string()));
        flatten_value(value, &mut values);
    }
    if values.is_empty() {
        Err("没有找到可导入的账号".to_string())
    } else {
        Ok(values)
    }
}

fn flatten_value(value: Value, output: &mut Vec<Value>) {
    match value {
        Value::Array(values) => {
            for value in values {
                flatten_value(value, output);
            }
        }
        Value::Object(mut object) => {
            if let Some(accounts) = object.remove("accounts") {
                flatten_value(accounts, output);
            } else if let Some(data) = object.get("data") {
                if data.get("accounts").is_some() {
                    flatten_value(data.clone(), output);
                } else {
                    output.push(Value::Object(object));
                }
            } else {
                output.push(Value::Object(object));
            }
        }
        other => output.push(other),
    }
}

fn account_from_value(value: Value) -> Result<NewAccount, String> {
    if let Some(token) = value.as_str() {
        return account_from_token(token);
    }
    let object = value
        .as_object()
        .ok_or_else(|| "账号条目必须是 JSON 对象或 token 字符串".to_string())?;
    let platform = first_string(&value, &[&["platform"]]);
    if !platform.is_empty() && !platform.eq_ignore_ascii_case("openai") {
        return Err(format!("暂不支持平台 {platform}"));
    }

    let declared_type = first_string(&value, &[&["type"], &["account_type"]]);
    let api_key = first_string(
        &value,
        &[
            &["credentials", "api_key"],
            &["api_key"],
            &["OPENAI_API_KEY"],
        ],
    );
    let is_api_key = matches!(
        declared_type.to_ascii_lowercase().as_str(),
        "apikey" | "api_key" | "key"
    ) || (!api_key.is_empty() && !declared_type.eq_ignore_ascii_case("oauth"));
    let priority = first_value(&value, &[&["priority"]])
        .filter(|value| !value.is_null())
        .map(parse_priority)
        .transpose()?;
    let models = first_value(&value, &[&["models"], &["credentials", "models"]])
        .filter(|value| !value.is_null())
        .map(parse_models)
        .transpose()?;
    let weight = first_value(&value, &[&["weight"]])
        .filter(|value| !value.is_null())
        .map(parse_weight)
        .transpose()?;
    let concurrency = first_value(&value, &[&["concurrency"]])
        .filter(|value| !value.is_null())
        .map(parse_concurrency)
        .transpose()?;

    let name = first_string(
        &value,
        &[&["name"], &["user", "name"], &["label"], &["meta", "label"]],
    );
    if is_api_key {
        if api_key.is_empty() {
            return Err("中转站缺少 api_key".to_string());
        }
        return Ok(NewAccount {
            name,
            account_type: "api_key".to_string(),
            api_key,
            priority,
            models,
            weight,
            concurrency,
            base_url: first_string(
                &value,
                &[&["credentials", "base_url"], &["base_url"], &["baseUrl"]],
            ),
            ..NewAccount::default()
        });
    }

    let access_token = first_string(
        &value,
        &[
            &["credentials", "access_token"],
            &["credentials", "accessToken"],
            &["tokens", "access_token"],
            &["tokens", "accessToken"],
            &["token", "access_token"],
            &["token", "accessToken"],
            &["access_token"],
            &["token"],
        ],
    );
    let refresh_token = first_string(
        &value,
        &[
            &["credentials", "refresh_token"],
            &["credentials", "refreshToken"],
            &["tokens", "refresh_token"],
            &["tokens", "refreshToken"],
            &["token", "refresh_token"],
            &["token", "refreshToken"],
            &["refresh_token"],
        ],
    );
    if access_token.is_empty() && refresh_token.is_empty() {
        let keys = object
            .keys()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "未找到 access_token 或 refresh_token（字段: {keys}）"
        ));
    }
    let id_token = first_string(
        &value,
        &[
            &["credentials", "id_token"],
            &["credentials", "idToken"],
            &["tokens", "id_token"],
            &["tokens", "idToken"],
            &["token", "id_token"],
            &["token", "idToken"],
            &["id_token"],
        ],
    );
    let mut metadata = decode_token_metadata(&access_token);
    merge_metadata(&mut metadata, decode_token_metadata(&id_token));

    let explicit_token_expires = first_value(
        &value,
        &[
            &["credentials", "expires_at"],
            &["tokens", "expires_at"],
            &["tokens", "expiresAt"],
            &["expires_at"],
            &["expiresAt"],
            &["expired"],
        ],
    )
    .and_then(parse_time);
    let email = non_empty_or(
        first_string(
            &value,
            &[
                &["credentials", "email"],
                &["email"],
                &["meta", "label"],
                &["label"],
            ],
        ),
        &metadata.email,
    );
    let name = non_empty_or(name, &email);
    let expires_at = explicit_token_expires.or(metadata.expires_at);

    Ok(NewAccount {
        name,
        account_type: "oauth".to_string(),
        access_token,
        refresh_token,
        id_token,
        client_id: non_empty_or(
            first_string(
                &value,
                &[
                    &["credentials", "client_id"],
                    &["credentials", "clientId"],
                    &["tokens", "client_id"],
                    &["tokens", "clientId"],
                    &["token", "client_id"],
                    &["token", "clientId"],
                    &["client_id"],
                    &["clientId"],
                ],
            ),
            OPENAI_CLIENT_ID,
        ),
        chatgpt_account_id: non_empty_or(
            first_string(
                &value,
                &[
                    &["credentials", "chatgpt_account_id"],
                    &["credentials", "chatgptAccountId"],
                    &["tokens", "account_id"],
                    &["tokens", "accountId"],
                    &["tokens", "chatgpt_account_id"],
                    &["tokens", "chatgptAccountId"],
                    &["token", "account_id"],
                    &["token", "accountId"],
                    &["token", "chatgpt_account_id"],
                    &["token", "chatgptAccountId"],
                    &["chatgpt_account_id"],
                    &["chatgptAccountId"],
                    &["account_id"],
                    &["accountId"],
                    &["meta", "chatgpt_account_id"],
                    &["meta", "chatgptAccountId"],
                    &["credentials", "organization_id"],
                    &["credentials", "organizationId"],
                ],
            ),
            &metadata.chatgpt_account_id,
        ),
        chatgpt_user_id: non_empty_or(
            first_string(
                &value,
                &[
                    &["credentials", "chatgpt_user_id"],
                    &["credentials", "chatgptUserId"],
                    &["chatgpt_user_id"],
                    &["chatgptUserId"],
                    &["user_id"],
                    &["userId"],
                ],
            ),
            &metadata.chatgpt_user_id,
        ),
        email,
        plan_type: non_empty_or(
            first_string(
                &value,
                &[
                    &["credentials", "plan_type"],
                    &["plan_type"],
                    &["planType"],
                    &["chatgpt_plan_type"],
                    &["chatgptPlanType"],
                ],
            ),
            &metadata.plan_type,
        ),
        expires_at,
        priority,
        models,
        weight,
        concurrency,
        ..NewAccount::default()
    })
}

fn account_from_token(token: &str) -> Result<NewAccount, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("token 为空".to_string());
    }
    if token.starts_with("sk-") {
        return Ok(NewAccount {
            account_type: "api_key".to_string(),
            api_key: token.to_string(),
            ..NewAccount::default()
        });
    }
    let metadata = decode_token_metadata(token);
    Ok(NewAccount {
        name: metadata.email.clone(),
        account_type: "oauth".to_string(),
        access_token: token.to_string(),
        client_id: OPENAI_CLIENT_ID.to_string(),
        chatgpt_account_id: metadata.chatgpt_account_id,
        chatgpt_user_id: metadata.chatgpt_user_id,
        email: metadata.email,
        plan_type: metadata.plan_type,
        expires_at: metadata.expires_at,
        ..NewAccount::default()
    })
}

fn first_string(value: &Value, paths: &[&[&str]]) -> String {
    paths
        .iter()
        .find_map(|path| {
            value_at(value, path)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn first_value<'a>(value: &'a Value, paths: &[&[&str]]) -> Option<&'a Value> {
    paths.iter().find_map(|path| value_at(value, path))
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    Some(cursor)
}

fn parse_time(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(if number > 10_000_000_000 {
            number / 1000
        } else {
            number
        });
    }
    let raw = value.as_str()?.trim();
    raw.parse::<i64>()
        .ok()
        .map(|number| {
            if number > 10_000_000_000 {
                number / 1000
            } else {
                number
            }
        })
        .or_else(|| {
            DateTime::parse_from_rfc3339(raw)
                .ok()
                .map(|time| time.timestamp())
        })
}

fn parse_priority(value: &Value) -> Result<i64, String> {
    let parsed = value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|raw| raw.trim().parse::<i64>().ok())
        })
        .ok_or_else(|| "priority 必须是整数".to_string())?;
    Ok(parsed.clamp(0, 1000))
}

fn parse_models(value: &Value) -> Result<Vec<String>, String> {
    let models = match value {
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| "models 必须是字符串数组".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        Value::String(value) => value
            .split(|character| character == ',' || character == '\n')
            .collect(),
        _ => return Err("models 必须是字符串数组或逗号分隔字符串".to_string()),
    };
    Ok(normalize_models(models))
}

fn parse_weight(value: &Value) -> Result<i64, String> {
    let parsed = value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|raw| raw.trim().parse::<i64>().ok())
        })
        .ok_or_else(|| "weight 必须是正整数".to_string())?;
    if !(1..=1000).contains(&parsed) {
        return Err("weight 必须在 1 到 1000 之间".to_string());
    }
    Ok(parsed)
}

fn parse_concurrency(value: &Value) -> Result<i64, String> {
    let parsed = value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|raw| raw.trim().parse::<i64>().ok())
        })
        .ok_or_else(|| "concurrency 必须是正整数".to_string())?;
    if !(1..=1000).contains(&parsed) {
        return Err("concurrency 必须在 1 到 1000 之间".to_string());
    }
    Ok(parsed)
}

fn non_empty_or(value: String, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_auth_and_sub2api_backup() {
        let contents = vec![
            r#"{"tokens":{"access_token":"ey.token.sig","refresh_token":"rt-test","account_id":"acc-1"}}"#.to_string(),
            r#"{"type":"sub2api-data","accounts":[{"name":"key","platform":"openai","type":"apikey","priority":7,"models":[" gpt-5 ","gpt-5","gpt-5-mini"],"weight":"3","credentials":{"api_key":"sk-test","base_url":"https://example.com/v1"}}]}"#.to_string(),
        ];
        let (accounts, errors) = parse_import_contents(&contents);
        assert!(errors.is_empty());
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].chatgpt_account_id, "acc-1");
        assert_eq!(accounts[1].api_key, "sk-test");
        assert_eq!(accounts[1].priority, Some(7));
        assert_eq!(
            accounts[1].models,
            Some(vec!["gpt-5".to_string(), "gpt-5-mini".to_string()])
        );
        assert_eq!(accounts[1].weight, Some(3));
        assert_eq!(accounts[0].models, None);
        assert_eq!(accounts[0].weight, None);
    }

    #[test]
    fn parses_empty_allowlist_and_rejects_invalid_weight() {
        let contents = vec![
            r#"{"type":"api_key","api_key":"sk-all","models":"","weight":1}"#.to_string(),
            r#"{"type":"api_key","api_key":"sk-invalid","weight":0}"#.to_string(),
        ];

        let (accounts, errors) = parse_import_contents(&contents);

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].models, Some(Vec::new()));
        assert_eq!(accounts[0].weight, Some(1));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("weight 必须在 1 到 1000 之间"));
    }

    #[test]
    fn parses_cpa_codex_account() {
        let contents = vec![r#"{
            "type":"codex",
            "access_token":"access-test",
            "refresh_token":"refresh-test",
            "id_token":"id-test",
            "account_id":"account-test",
            "email":"person@example.com",
            "chatgpt_plan_type":"plus",
            "expired":"2026-08-06T14:29:36.155Z"
        }"#
        .to_string()];

        let (accounts, errors) = parse_import_contents(&contents);

        assert!(errors.is_empty());
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "person@example.com");
        assert_eq!(accounts[0].account_type, "oauth");
        assert_eq!(accounts[0].access_token, "access-test");
        assert_eq!(accounts[0].refresh_token, "refresh-test");
        assert_eq!(accounts[0].id_token, "id-test");
        assert_eq!(accounts[0].chatgpt_account_id, "account-test");
        assert_eq!(accounts[0].plan_type, "plus");
        assert_eq!(accounts[0].expires_at, Some(1_786_026_576));
    }

    #[test]
    fn recognizes_complete_cpa_clipboard_json() {
        let parsed = parse_clipboard_import(
            r#"[
                {
                    "type":"codex",
                    "access_token":"access-one",
                    "refresh_token":"refresh-one",
                    "email":"one@example.com"
                },
                {
                    "type":"codex",
                    "refresh_token":"refresh-two",
                    "email":"two@example.com"
                }
            ]"#,
        )
        .unwrap();

        assert_eq!(parsed.source, ClipboardImportSource::Cpa);
        assert_eq!(parsed.accounts.len(), 2);
        assert_eq!(parsed.accounts[0].email, "one@example.com");
        assert_eq!(parsed.accounts[1].refresh_token, "refresh-two");
    }

    #[test]
    fn recognizes_sub2api_account_and_backup_clipboard_json() {
        let single = parse_clipboard_import(
            r#"{
                "name":"OAuth one",
                "platform":"openai",
                "type":"oauth",
                "credentials":{
                    "access_token":"access-one",
                    "refresh_token":"refresh-one",
                    "email":"one@example.com"
                }
            }"#,
        )
        .unwrap();
        assert_eq!(single.source, ClipboardImportSource::Sub2api);
        assert_eq!(single.accounts.len(), 1);

        let backup = parse_clipboard_import(
            r#"{
                "type":"sub2api-data",
                "version":1,
                "accounts":[
                    {
                        "name":"OAuth one",
                        "platform":"openai",
                        "type":"oauth",
                        "credentials":{"refresh_token":"refresh-one"}
                    },
                    {
                        "name":"Relay one",
                        "platform":"openai",
                        "type":"api_key",
                        "concurrency":24,
                        "credentials":{"api_key":"sk-test","base_url":"https://example.com/v1"}
                    }
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(backup.source, ClipboardImportSource::Sub2api);
        assert_eq!(backup.accounts.len(), 2);
        assert_eq!(backup.accounts[1].account_type, "api_key");
        assert_eq!(backup.accounts[1].concurrency, Some(24));
    }

    #[test]
    fn clipboard_import_rejects_partial_mixed_and_loose_content() {
        let invalid_values = [
            r#"{"type":"codex","email":"missing@example.com"}"#,
            r#"{"platform":"openai","type":"oauth","access_token":"flat"}"#,
            r#"{"type":"sub2api-data","accounts":[{"platform":"openai","type":"oauth","credentials":{"refresh_token":"ok"}},{"type":"codex","refresh_token":"mixed"}]}"#,
            r#"{"type":"sub2api-data","accounts":[{"platform":"openai","type":"oauth","credentials":{"refresh_token":"ok"}},{"platform":"openai","type":"api_key","credentials":{}}]}"#,
            "sk-not-json",
        ];

        for value in invalid_values {
            assert!(parse_clipboard_import(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn clipboard_import_accepts_multiple_cpa_objects_with_noise() {
        let content = "=== 卡密内容 ===\n{\"type\":\"codex\",\"access_token\":\"a1\",\"refresh_token\":\"r1\",\"email\":\"one@example.com\"}\n{\"type\":\"codex\",\"access_token\":\"a2\",\"refresh_token\":\"r2\",\"email\":\"two@example.com\"}\n=== 结束 ===";
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.source, ClipboardImportSource::Cpa);
        assert_eq!(parsed.accounts.len(), 2);
        assert_eq!(parsed.accounts[0].email, "one@example.com");
        assert_eq!(parsed.accounts[1].email, "two@example.com");
    }

    #[test]
    fn clipboard_import_accepts_multiple_sub2api_objects_with_noise() {
        let content = "=== 卡密内容 ===\n{\"name\":\"a\",\"platform\":\"openai\",\"type\":\"oauth\",\"credentials\":{\"refresh_token\":\"r1\",\"email\":\"one@example.com\"}}\n{\"name\":\"b\",\"platform\":\"openai\",\"type\":\"oauth\",\"credentials\":{\"refresh_token\":\"r2\",\"email\":\"two@example.com\"}}";
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.source, ClipboardImportSource::Sub2api);
        assert_eq!(parsed.accounts.len(), 2);
    }

    #[test]
    fn clipboard_import_extracts_json_array_from_noise() {
        let content = "some header text\n[{\"type\":\"codex\",\"refresh_token\":\"r1\",\"email\":\"a@b.com\"},{\"type\":\"codex\",\"refresh_token\":\"r2\",\"email\":\"c@d.com\"}]\nfooter";
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.source, ClipboardImportSource::Cpa);
        assert_eq!(parsed.accounts.len(), 2);
    }

    #[test]
    fn clipboard_import_rejects_mixed_cpa_and_sub2api() {
        let content = "{\"type\":\"codex\",\"refresh_token\":\"r1\"}\n{\"name\":\"x\",\"platform\":\"openai\",\"type\":\"oauth\",\"credentials\":{\"refresh_token\":\"r2\"}}";
        assert!(parse_clipboard_import(content).is_err());
    }

    #[test]
    fn rejects_chatgpt_web_session() {
        let contents = vec![r#"{
            "user":{"id":"user-test","email":"person@example.com"},
            "account":{"id":"account-test","planType":"team"},
            "accessToken":"access-test",
            "refreshToken":"refresh-test",
            "idToken":"id-test",
            "expires":"2026-08-06T14:29:36.155Z"
        }"#
        .to_string()];

        let (accounts, errors) = parse_import_contents(&contents);

        assert!(accounts.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0]
            .message
            .contains("未找到 access_token 或 refresh_token"));
    }
}
