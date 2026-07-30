use crate::db::{Account, NewAccount};
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;

pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REFRESH_SCOPE: &str = "openid profile email";

#[derive(Debug, Clone, Default)]
pub struct TokenMetadata {
    pub email: String,
    pub chatgpt_account_id: String,
    pub chatgpt_user_id: String,
    pub plan_type: String,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    expires_in: i64,
}

pub async fn refresh_account(
    client: &reqwest::Client,
    account: &Account,
) -> Result<NewAccount, String> {
    if account.account_type != "oauth" {
        return Err("该账号不是 OAuth 账号".to_string());
    }
    refresh_new_account(
        client,
        &NewAccount {
            name: account.name.clone(),
            account_type: account.account_type.clone(),
            access_token: account.access_token.clone(),
            refresh_token: account.refresh_token.clone(),
            id_token: account.id_token.clone(),
            client_id: account.client_id.clone(),
            chatgpt_account_id: account.chatgpt_account_id.clone(),
            chatgpt_user_id: account.chatgpt_user_id.clone(),
            email: account.email.clone(),
            plan_type: account.plan_type.clone(),
            expires_at: account.expires_at,
            priority: Some(account.priority),
            ..NewAccount::default()
        },
    )
    .await
}

pub async fn refresh_new_account(
    client: &reqwest::Client,
    account: &NewAccount,
) -> Result<NewAccount, String> {
    if account.refresh_token.trim().is_empty() {
        return Err("账号没有 refresh_token，无法自动续期".to_string());
    }

    let client_id = if account.client_id.trim().is_empty() {
        OPENAI_CLIENT_ID
    } else {
        account.client_id.trim()
    };
    let response = client
        .post(OPENAI_TOKEN_URL)
        .header("User-Agent", "codex-cli/0.144.1")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", account.refresh_token.as_str()),
            ("client_id", client_id),
            ("scope", REFRESH_SCOPE),
        ])
        .send()
        .await
        .map_err(|error| format!("连接 OpenAI OAuth 服务失败: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(format!(
            "OpenAI OAuth 刷新失败 ({status}): {}",
            oauth_error_message(&detail)
        ));
    }

    let token: TokenResponse = response
        .json()
        .await
        .map_err(|error| format!("OpenAI OAuth 返回内容无效: {error}"))?;
    if token.access_token.trim().is_empty() {
        return Err("OpenAI OAuth 未返回 access_token".to_string());
    }

    let mut metadata = decode_token_metadata(&token.access_token);
    merge_metadata(&mut metadata, decode_token_metadata(&token.id_token));
    if metadata.expires_at.is_none() && token.expires_in > 0 {
        metadata.expires_at = Some(chrono::Utc::now().timestamp() + token.expires_in);
    }

    Ok(NewAccount {
        name: account.name.clone(),
        account_type: "oauth".to_string(),
        access_token: token.access_token,
        refresh_token: if token.refresh_token.is_empty() {
            account.refresh_token.clone()
        } else {
            token.refresh_token
        },
        id_token: if token.id_token.is_empty() {
            account.id_token.clone()
        } else {
            token.id_token
        },
        client_id: client_id.to_string(),
        chatgpt_account_id: fallback(metadata.chatgpt_account_id, &account.chatgpt_account_id),
        chatgpt_user_id: fallback(metadata.chatgpt_user_id, &account.chatgpt_user_id),
        email: fallback(metadata.email, &account.email),
        plan_type: fallback(metadata.plan_type, &account.plan_type),
        expires_at: metadata.expires_at.or(account.expires_at),
        priority: account.priority,
        ..NewAccount::default()
    })
}

pub fn decode_token_metadata(token: &str) -> TokenMetadata {
    let mut result = TokenMetadata::default();
    let payload = match token.split('.').nth(1) {
        Some(value) => value,
        None => return result,
    };
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload));
    let Ok(decoded) = decoded else {
        return result;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&decoded) else {
        return result;
    };

    result.email = string_at(&value, &["email"]);
    result.expires_at = value.get("exp").and_then(Value::as_i64);
    if result.chatgpt_user_id.is_empty() {
        result.chatgpt_user_id = string_at(&value, &["sub"]);
    }
    if let Some(auth) = value
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object)
    {
        result.chatgpt_account_id = object_string(auth, "chatgpt_account_id");
        result.chatgpt_user_id = first_non_empty([
            object_string(auth, "chatgpt_user_id"),
            object_string(auth, "user_id"),
            result.chatgpt_user_id,
        ]);
        result.plan_type = object_string(auth, "chatgpt_plan_type");
    }
    result
}

pub fn merge_metadata(target: &mut TokenMetadata, source: TokenMetadata) {
    if target.email.is_empty() {
        target.email = source.email;
    }
    if target.chatgpt_account_id.is_empty() {
        target.chatgpt_account_id = source.chatgpt_account_id;
    }
    if target.chatgpt_user_id.is_empty() {
        target.chatgpt_user_id = source.chatgpt_user_id;
    }
    if target.plan_type.is_empty() {
        target.plan_type = source.plan_type;
    }
    if target.expires_at.is_none() {
        target.expires_at = source.expires_at;
    }
}

fn string_at(value: &Value, path: &[&str]) -> String {
    let mut cursor = value;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return String::new();
        };
        cursor = next;
    }
    cursor.as_str().unwrap_or_default().trim().to_string()
}

fn object_string(object: &serde_json::Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn first_non_empty<const N: usize>(values: [String; N]) -> String {
    values
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn fallback(value: String, current: &str) -> String {
    if value.is_empty() {
        current.to_string()
    } else {
        value
    }
}

fn oauth_error_message(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| {
            value
                .get("error_description")
                .or_else(|| value.get("error"))
                .and_then(Value::as_str)
        })
        .unwrap_or("上游拒绝了刷新请求");
    message.chars().take(300).collect()
}
