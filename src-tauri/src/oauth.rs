use crate::db::{Account, NewAccount};
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const REFRESH_SCOPE: &str = "openid profile email";
const AUTHORIZE_SCOPE: &str = "openid profile email offline_access";
const OAUTH_SESSION_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_OAUTH_SESSIONS: usize = 12;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIAuthorization {
    pub auth_url: String,
    pub session_id: String,
    pub state: String,
}

#[derive(Debug, Clone)]
struct PendingAuthorization {
    name: String,
    priority: i64,
    state: String,
    code_verifier: String,
    created_at: Instant,
}

/// Short-lived, in-memory PKCE sessions used by the manual OpenAI authorization dialog.
pub struct OpenAIOAuthSessions {
    pending: Mutex<HashMap<String, PendingAuthorization>>,
}

impl Default for OpenAIOAuthSessions {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }
}

impl OpenAIOAuthSessions {
    pub fn begin(&self, name: String, priority: i64) -> Result<OpenAIAuthorization, String> {
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let state = uuid::Uuid::new_v4().simple().to_string();
        // The Codex client uses a hex PKCE verifier. Two UUIDs give the required
        // entropy and stay within OAuth's 43-128 character verifier window.
        let code_verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(code_verifier.as_bytes()));
        let auth_url = reqwest::Url::parse_with_params(
            OPENAI_AUTHORIZE_URL,
            &[
                ("response_type", "code"),
                ("client_id", OPENAI_CLIENT_ID),
                ("redirect_uri", OPENAI_REDIRECT_URI),
                ("scope", AUTHORIZE_SCOPE),
                ("state", state.as_str()),
                ("code_challenge", challenge.as_str()),
                ("code_challenge_method", "S256"),
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
            ],
        )
        .map_err(|error| format!("生成 OpenAI 授权链接失败: {error}"))?
        .to_string();
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, value| value.created_at.elapsed() < OAUTH_SESSION_TTL);
        if pending.len() >= MAX_OAUTH_SESSIONS {
            if let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, value)| value.created_at)
                .map(|(id, _)| id.clone())
            {
                pending.remove(&oldest);
            }
        }
        pending.insert(
            session_id.clone(),
            PendingAuthorization {
                name,
                priority,
                state: state.clone(),
                code_verifier,
                created_at: Instant::now(),
            },
        );
        Ok(OpenAIAuthorization {
            auth_url,
            session_id,
            state,
        })
    }

    pub async fn complete(
        &self,
        client: &reqwest::Client,
        session_id: &str,
        callback_or_code: &str,
        codex_version: &str,
    ) -> Result<NewAccount, String> {
        let session = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending.retain(|_, value| value.created_at.elapsed() < OAUTH_SESSION_TTL);
            pending
                .get(session_id)
                .cloned()
                .ok_or_else(|| "授权已过期，请重新生成授权链接".to_string())?
        };
        let (code, returned_state) = parse_callback_or_code(callback_or_code)?;
        if let Some(returned_state) = returned_state {
            if returned_state != session.state {
                return Err("授权回调与当前授权链接不匹配，请重新开始授权".to_string());
            }
        }

        let response = client
            .post(OPENAI_TOKEN_URL)
            .header("User-Agent", crate::codex_identity::user_agent(codex_version))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", OPENAI_CLIENT_ID),
                ("code", code.as_str()),
                ("redirect_uri", OPENAI_REDIRECT_URI),
                ("code_verifier", session.code_verifier.as_str()),
            ])
            .send()
            .await
            .map_err(|error| format!("连接 OpenAI OAuth 服务失败: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(format!(
                "OpenAI 授权码兑换失败 ({status}): {}",
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
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
        Ok(NewAccount {
            name: session.name,
            account_type: "oauth".to_string(),
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            id_token: token.id_token,
            client_id: OPENAI_CLIENT_ID.to_string(),
            chatgpt_account_id: metadata.chatgpt_account_id,
            chatgpt_user_id: metadata.chatgpt_user_id,
            email: metadata.email,
            plan_type: metadata.plan_type,
            expires_at: metadata.expires_at,
            priority: Some(session.priority),
            ..NewAccount::default()
        })
    }

    pub async fn complete_callback(
        &self,
        client: &reqwest::Client,
        code: &str,
        returned_state: &str,
        codex_version: &str,
    ) -> Result<(String, NewAccount), String> {
        let returned_state = returned_state.trim();
        if returned_state.is_empty() {
            return Err("授权回调缺少 state 参数".to_string());
        }
        let session_id = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending.retain(|_, value| value.created_at.elapsed() < OAUTH_SESSION_TTL);
            pending
                .iter()
                .find_map(|(id, value)| (value.state == returned_state).then(|| id.clone()))
                .ok_or_else(|| "授权已过期或不属于当前应用，请重新开始授权".to_string())?
        };
        let account = self
            .complete(client, &session_id, code, codex_version)
            .await?;
        Ok((session_id, account))
    }
}

fn parse_callback_or_code(input: &str) -> Result<(String, Option<String>), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("请粘贴授权回调链接或 code".to_string());
    }
    let Ok(url) = reqwest::Url::parse(input) else {
        return Ok((input.to_string(), None));
    };
    let code = url
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "授权回调链接缺少 code 参数".to_string())?;
    let returned_state = url
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()));
    Ok((code, returned_state))
}

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
    codex_version: &str,
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
        codex_version,
    )
    .await
}

pub async fn refresh_new_account(
    client: &reqwest::Client,
    account: &NewAccount,
    codex_version: &str,
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
        .header("User-Agent", crate::codex_identity::user_agent(codex_version))
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
