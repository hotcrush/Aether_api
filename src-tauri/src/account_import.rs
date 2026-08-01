use crate::db::{normalize_models, NewAccount};
use crate::oauth::{decode_token_metadata, merge_metadata, OPENAI_CLIENT_ID};
use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;
const MAX_AUTO_IMPORT_STRUCTURAL_MARKERS: usize = 262_144;
const MAX_EMBEDDED_JSON_VALUES: usize = 4_096;
const MAX_EMBEDDED_JSON_SCAN_ATTEMPTS: usize = 16_384;
const MAX_FAILED_JSON_SCAN_WORK_MULTIPLIER: usize = 4;
const MAX_DETECTED_IMPORT_ACCOUNTS: usize = 4_096;

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
    if trimmed
        .bytes()
        .filter(|byte| matches!(byte, b'{' | b'}' | b'[' | b']' | b',' | b':'))
        .take(MAX_AUTO_IMPORT_STRUCTURAL_MARKERS + 1)
        .count()
        > MAX_AUTO_IMPORT_STRUCTURAL_MARKERS
    {
        return Err("自动导入内容的 JSON 结构过于复杂".to_string());
    }

    let extracted = extract_json_values(trimmed)?;
    let mut source = None;
    let mut account_values = Vec::new();
    for value in extracted {
        let Some(detected) = collect_classified_account_values(value, &mut account_values)? else {
            continue;
        };
        if source.is_some_and(|existing| existing != detected) {
            return Err("自动导入内容同时包含 CPA 和 Sub2API JSON，请分开导入".to_string());
        }
        source = Some(detected);
    }

    let source = source.ok_or_else(|| "没有找到受支持的 CPA 或 Sub2API JSON".to_string())?;
    let mut accounts = Vec::with_capacity(account_values.len());
    for value in account_values {
        accounts.push(account_from_classified_value(value, source)?);
    }
    if accounts.is_empty() {
        return Err("没有找到可导入的账号".to_string());
    }
    Ok(ParsedClipboardImport { source, accounts })
}

fn account_from_classified_value(
    mut value: Value,
    source: ClipboardImportSource,
) -> Result<NewAccount, String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "自动识别的账号条目必须是 JSON 对象".to_string())?;
    let allowed_fields: &[&str] = match source {
        ClipboardImportSource::Cpa => &[
            "type",
            "platform",
            "name",
            "access_token",
            "refresh_token",
            "id_token",
            "client_id",
            "clientId",
            "chatgpt_account_id",
            "chatgptAccountId",
            "account_id",
            "accountId",
            "chatgpt_user_id",
            "chatgptUserId",
            "user_id",
            "userId",
            "email",
            "plan_type",
            "planType",
            "chatgpt_plan_type",
            "chatgptPlanType",
            "expires_at",
            "expiresAt",
            "expired",
            "priority",
            "models",
            "weight",
            "concurrency",
        ],
        ClipboardImportSource::Sub2api => &[
            "name",
            "platform",
            "type",
            "credentials",
            "priority",
            "models",
            "weight",
            "concurrency",
        ],
    };
    object.retain(|field, _| allowed_fields.contains(&field.as_str()));

    if source == ClipboardImportSource::Sub2api {
        let credentials = object
            .get_mut("credentials")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "Sub2API 账号缺少 credentials 对象".to_string())?;
        const ALLOWED_CREDENTIAL_FIELDS: &[&str] = &[
            "api_key",
            "base_url",
            "access_token",
            "accessToken",
            "refresh_token",
            "refreshToken",
            "id_token",
            "idToken",
            "client_id",
            "clientId",
            "chatgpt_account_id",
            "chatgptAccountId",
            "account_id",
            "accountId",
            "organization_id",
            "organizationId",
            "chatgpt_user_id",
            "chatgptUserId",
            "user_id",
            "userId",
            "email",
            "plan_type",
            "expires_at",
            "models",
        ];
        credentials.retain(|field, _| ALLOWED_CREDENTIAL_FIELDS.contains(&field.as_str()));
    }

    let account = account_from_value(value)?;
    if source == ClipboardImportSource::Cpa && account.account_type != "oauth" {
        return Err("CPA JSON 账号类型与凭据不一致".to_string());
    }
    Ok(account)
}

fn collect_classified_account_values(
    value: Value,
    accounts: &mut Vec<Value>,
) -> Result<Option<ClipboardImportSource>, String> {
    if is_cpa_account(&value) {
        push_detected_account(accounts, value)?;
        return Ok(Some(ClipboardImportSource::Cpa));
    }
    if is_sub2api_account(&value) {
        push_detected_account(accounts, value)?;
        return Ok(Some(ClipboardImportSource::Sub2api));
    }
    if is_sub2api_backup(&value) {
        let backup_accounts = take_sub2api_backup_accounts(value)
            .ok_or_else(|| "Sub2API 备份缺少 accounts 数组".to_string())?;
        let detected = collect_classified_account_values(backup_accounts, accounts)?;
        if detected != Some(ClipboardImportSource::Sub2api) {
            return Err("Sub2API 备份包含无法识别的账号条目".to_string());
        }
        return Ok(detected);
    }
    if let Value::Array(values) = value {
        let mut source = None;
        let mut contains_unsupported = false;
        for item in values {
            match collect_classified_account_values(item, accounts)? {
                Some(detected) => {
                    if source.is_some_and(|existing| existing != detected) {
                        return Err(
                            "自动导入 JSON 数组同时包含 CPA 和 Sub2API，请分开导入".to_string()
                        );
                    }
                    source = Some(detected);
                }
                None => contains_unsupported = true,
            }
        }
        if source.is_some() && contains_unsupported {
            return Err("自动导入 JSON 数组包含无法识别的账号条目".to_string());
        }
        return Ok(source);
    }
    if looks_like_supported_account(&value) {
        return Err("CPA 或 Sub2API JSON 账号字段不完整".to_string());
    }
    Ok(None)
}

fn push_detected_account(accounts: &mut Vec<Value>, value: Value) -> Result<(), String> {
    if accounts.len() >= MAX_DETECTED_IMPORT_ACCOUNTS {
        return Err("自动识别导入的账号数量不能超过 4096 个".to_string());
    }
    accounts.push(value);
    Ok(())
}

fn take_sub2api_backup_accounts(mut value: Value) -> Option<Value> {
    let object = value.as_object_mut()?;
    if let Some(accounts) = object.remove("accounts") {
        return Some(accounts);
    }
    object.get_mut("data")?.as_object_mut()?.remove("accounts")
}

fn looks_like_supported_account(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    string_field_eq(object.get("type"), "codex")
        || string_field_eq(object.get("type"), "sub2api-data")
        || (string_field_eq(object.get("platform"), "openai")
            && object
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|account_type| {
                    matches!(
                        account_type.to_ascii_lowercase().as_str(),
                        "oauth" | "apikey" | "api_key" | "key"
                    )
                }))
}

fn extract_json_values(text: &str) -> Result<Vec<Value>, String> {
    let mut values = Vec::new();
    let mut offset = 0;
    let mut attempts = 0;
    let mut failed_scan_work = 0usize;
    let failed_scan_budget = text
        .len()
        .saturating_mul(MAX_FAILED_JSON_SCAN_WORK_MULTIPLIER);

    while offset < text.len() {
        let Some((relative_start, opening)) = text[offset..]
            .char_indices()
            .find(|(_, character)| matches!(character, '{' | '['))
        else {
            break;
        };
        let start = offset + relative_start;
        attempts += 1;
        if attempts > MAX_EMBEDDED_JSON_SCAN_ATTEMPTS {
            return Err("自动导入内容中的 JSON 候选过多".to_string());
        }

        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                let consumed = stream.byte_offset();
                if consumed == 0 {
                    offset = start + opening.len_utf8();
                    continue;
                }
                values.push(value);
                if values.len() > MAX_EMBEDDED_JSON_VALUES {
                    return Err("自动导入内容中的完整 JSON 数量不能超过 4096 个".to_string());
                }
                offset = start + consumed;
            }
            Some(Err(error)) => {
                failed_scan_work = failed_scan_work
                    .saturating_add(json_error_scan_distance(&text[start..], &error));
                if failed_scan_work > failed_scan_budget {
                    return Err("自动导入内容中的无效 JSON 候选过于复杂".to_string());
                }
                offset = start + opening.len_utf8();
            }
            None => offset = start + opening.len_utf8(),
        }
    }

    Ok(values)
}

fn json_error_scan_distance(candidate: &str, error: &serde_json::Error) -> usize {
    let target_line = error.line().max(1);
    let mut current_line = 1;
    let mut line_start = 0;
    if target_line > 1 {
        for (index, byte) in candidate.bytes().enumerate() {
            if byte != b'\n' {
                continue;
            }
            current_line += 1;
            line_start = index + 1;
            if current_line == target_line {
                break;
            }
        }
    }
    line_start
        .saturating_add(error.column())
        .clamp(1, candidate.len())
}

fn is_cpa_account(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    string_field_eq(object.get("type"), "codex")
        && !object.contains_key("credentials")
        && !has_non_empty_string(object.get("api_key"), object.get("OPENAI_API_KEY"))
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
    let Some(credentials) = object.get("credentials").and_then(Value::as_object) else {
        return false;
    };
    if !string_field_eq(object.get("platform"), "openai") {
        return false;
    }
    match account_type.to_ascii_lowercase().as_str() {
        "oauth" => has_non_empty_string(
            credentials.get("access_token"),
            credentials.get("refresh_token"),
        ),
        "apikey" | "api_key" | "key" => has_non_empty_string(credentials.get("api_key"), None),
        _ => false,
    }
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
        .ok_or_else(|| "concurrency 必须是非负整数".to_string())?;
    if parsed < 0 {
        return Err("concurrency 必须是非负整数".to_string());
    }
    // Sub2API uses zero for unlimited concurrency and permits limits above
    // Aether's supported range. Saturate both cases at the local maximum.
    Ok(if parsed == 0 { 1000 } else { parsed.min(1000) })
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
    fn imports_unlimited_concurrency_as_supported_max_for_multiple_accounts() {
        let contents = vec![r#"{
            "type":"sub2api-data",
            "accounts":[
                {
                    "name":"first",
                    "platform":"openai",
                    "type":"oauth",
                    "concurrency":0,
                    "credentials":{"refresh_token":"refresh-one"}
                },
                {
                    "name":"second",
                    "platform":"openai",
                    "type":"oauth",
                    "concurrency":"0",
                    "credentials":{"refresh_token":"refresh-two"}
                }
            ]
        }"#
        .to_string()];

        let (accounts, errors) = parse_import_contents(&contents);

        assert!(errors.is_empty());
        assert_eq!(accounts.len(), 2);
        assert!(accounts
            .iter()
            .all(|account| account.concurrency == Some(1000)));
    }

    #[test]
    fn clamps_large_import_concurrency_and_rejects_negative_values() {
        assert_eq!(parse_concurrency(&serde_json::json!(1001)), Ok(1000));
        assert!(parse_concurrency(&serde_json::json!(-1)).is_err());
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
    fn clipboard_import_recovers_after_malformed_bracket_noise() {
        let content = r#"
            商品说明：[这不是完整 JSON
            卡密：{"name":"valid","platform":"openai","type":"oauth","credentials":{"refresh_token":"refresh-valid"}}
        "#;
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.source, ClipboardImportSource::Sub2api);
        assert_eq!(parsed.accounts.len(), 1);
        assert_eq!(parsed.accounts[0].refresh_token, "refresh-valid");
    }

    #[test]
    fn embedded_json_failed_scan_work_is_bounded() {
        let content = format!("{}\"unfinished\"", "{\"nested\":".repeat(9));
        let error = extract_json_values(&content).unwrap_err();
        assert_eq!(error, "自动导入内容中的无效 JSON 候选过于复杂");
    }

    #[test]
    fn embedded_json_non_eof_failed_scan_work_is_bounded() {
        let content = format!(r#"{}"\uZZZZ""#, "{\"nested\":".repeat(12));
        let error = extract_json_values(&content).unwrap_err();
        assert_eq!(error, "自动导入内容中的无效 JSON 候选过于复杂");
    }

    #[test]
    fn clipboard_import_accepts_raw_sub2api_array() {
        let content = r#"[
            {"name":"a","platform":"openai","type":"oauth","credentials":{"refresh_token":"r1"}},
            {"name":"b","platform":"openai","type":"api_key","credentials":{"api_key":"sk-test","base_url":"https://example.com/v1"}}
        ]"#;
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.source, ClipboardImportSource::Sub2api);
        assert_eq!(parsed.accounts.len(), 2);
        assert_eq!(parsed.accounts[1].account_type, "api_key");
    }

    #[test]
    fn clipboard_import_discards_unrelated_json_without_importing_it() {
        let content = r#"
            订单信息：{"order":"A-100","access_token":"must-not-import"}
            卡密：{"name":"valid","platform":"openai","type":"oauth","credentials":{"refresh_token":"refresh-valid"}}
        "#;
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.source, ClipboardImportSource::Sub2api);
        assert_eq!(parsed.accounts.len(), 1);
        assert_eq!(parsed.accounts[0].refresh_token, "refresh-valid");
        assert_ne!(parsed.accounts[0].access_token, "must-not-import");
    }

    #[test]
    fn clipboard_import_rejects_supported_array_with_unknown_entries() {
        let content = r#"[
            {"type":"codex","refresh_token":"r1"},
            {"note":"not an account"}
        ]"#;
        assert!(parse_clipboard_import(content).is_err());
    }

    #[test]
    fn clipboard_import_does_not_expand_accounts_on_regular_account_objects() {
        let content = r#"{
            "type":"codex",
            "refresh_token":"outer-refresh",
            "accounts":[{"access_token":"must-not-import"}],
            "data":{"accounts":[{"access_token":"also-must-not-import"}]}
        }"#;
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.source, ClipboardImportSource::Cpa);
        assert_eq!(parsed.accounts.len(), 1);
        assert_eq!(parsed.accounts[0].refresh_token, "outer-refresh");
        assert!(parsed.accounts[0].access_token.is_empty());
    }

    #[test]
    fn clipboard_import_requires_sub2api_credentials_in_credentials_object() {
        let content = r#"{
            "platform":"openai",
            "type":"oauth",
            "refresh_token":"top-level-only",
            "credentials":{}
        }"#;
        assert!(parse_clipboard_import(content).is_err());
    }

    #[test]
    fn clipboard_import_rejects_cpa_with_conflicting_api_key() {
        let content = r#"{
            "type":"codex",
            "refresh_token":"oauth-refresh",
            "api_key":"sk-conflicting"
        }"#;
        assert!(parse_clipboard_import(content).is_err());
    }

    #[test]
    fn clipboard_import_uses_only_cpa_top_level_credentials() {
        let content = r#"{
            "type":"codex",
            "refresh_token":"checked-refresh",
            "tokens":{"refresh_token":"nested-noise"},
            "token":{"access_token":"nested-noise"},
            "user":{"name":"nested-noise"},
            "meta":{"label":"nested-noise","chatgpt_account_id":"nested-noise"},
            "label":"nested-noise"
        }"#;
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.accounts[0].refresh_token, "checked-refresh");
        assert!(parsed.accounts[0].access_token.is_empty());
        assert!(parsed.accounts[0].name.is_empty());
        assert!(parsed.accounts[0].chatgpt_account_id.is_empty());
    }

    #[test]
    fn clipboard_import_uses_only_sub2api_credentials_object() {
        let content = r#"{
            "platform":"openai",
            "type":"oauth",
            "access_token":"top-level-noise",
            "id_token":"top-level-noise",
            "account_id":"top-level-noise",
            "user_id":"top-level-noise",
            "email":"top-level-noise@example.com",
            "meta":{"label":"top-level-noise"},
            "label":"top-level-noise",
            "credentials":{
                "refresh_token":"checked-refresh",
                "chatgpt_account_id":"credential-account",
                "email":"credential@example.com"
            }
        }"#;
        let parsed = parse_clipboard_import(content).unwrap();
        assert_eq!(parsed.accounts[0].refresh_token, "checked-refresh");
        assert!(parsed.accounts[0].access_token.is_empty());
        assert!(parsed.accounts[0].id_token.is_empty());
        assert_eq!(parsed.accounts[0].chatgpt_account_id, "credential-account");
        assert!(parsed.accounts[0].chatgpt_user_id.is_empty());
        assert_eq!(parsed.accounts[0].email, "credential@example.com");
    }

    #[test]
    fn detected_import_account_count_is_bounded() {
        let value = Value::Array(
            (0..=MAX_DETECTED_IMPORT_ACCOUNTS)
                .map(|index| {
                    serde_json::json!({
                        "type": "codex",
                        "refresh_token": format!("refresh-{index}")
                    })
                })
                .collect(),
        );
        let mut accounts = Vec::new();
        assert!(collect_classified_account_values(value, &mut accounts).is_err());
        assert_eq!(accounts.len(), MAX_DETECTED_IMPORT_ACCOUNTS);
    }

    #[test]
    fn auto_import_json_structure_is_bounded_before_parsing() {
        let content = format!("[{}0]", "0,".repeat(MAX_AUTO_IMPORT_STRUCTURAL_MARKERS + 1));
        let error = parse_clipboard_import(&content).unwrap_err();
        assert_eq!(error, "自动导入内容的 JSON 结构过于复杂");
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
