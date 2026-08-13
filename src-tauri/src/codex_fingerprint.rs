use crate::db::{Account, Db};
use axum::http::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SETTINGS_KEY: &str = "codex_fingerprint_settings";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CodexFingerprintMode {
    Off,
    Device,
    #[default]
    Session,
    Full,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CodexFingerprintSettings {
    pub mode: CodexFingerprintMode,
}

impl Default for CodexFingerprintSettings {
    fn default() -> Self {
        Self {
            mode: CodexFingerprintMode::Session,
        }
    }
}

pub fn load(db: &Db) -> CodexFingerprintSettings {
    db.get_setting(SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save(
    db: &Db,
    settings: CodexFingerprintSettings,
) -> Result<CodexFingerprintSettings, String> {
    let encoded = serde_json::to_string(&settings).map_err(|error| error.to_string())?;
    db.set_setting(SETTINGS_KEY, &encoded)
        .map_err(|error| error.to_string())?;
    Ok(settings)
}

#[derive(Clone, Debug)]
pub struct PreparedRequest {
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Rewrites only Codex identity fields. The request is prepared once so the
/// same turn ID is used in both headers and client_metadata.
pub fn prepare(
    account: &Account,
    mode: CodexFingerprintMode,
    inbound_headers: &HeaderMap,
    body: &[u8],
) -> PreparedRequest {
    if mode == CodexFingerprintMode::Off || account.account_type != "oauth" {
        return PreparedRequest {
            headers: inbound_headers.clone(),
            body: body.to_vec(),
        };
    }

    let mut headers = inbound_headers.clone();
    if body.is_empty() {
        set_header(
            &mut headers,
            "x-codex-installation-id",
            &stable_id(&account.id, "installation"),
        );
        return PreparedRequest {
            headers,
            body: Vec::new(),
        };
    }
    let mut value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let object = value.as_object_mut();
    let original_session = first_non_empty([
        header_text(inbound_headers, "session-id"),
        header_text(inbound_headers, "session_id"),
    ])
    .unwrap_or_else(|| "default".to_string());

    let installation_id = stable_id(&account.id, "installation");
    set_header(&mut headers, "x-codex-installation-id", &installation_id);
    rewrite_header_turn_metadata(
        &mut headers,
        &[("installation_id", installation_id.as_str())],
    );
    if mode == CodexFingerprintMode::Device {
        if object.is_none() {
            return PreparedRequest {
                headers,
                body: body.to_vec(),
            };
        }
        if let Some(object) = object {
            rewrite_client_metadata(object, mode, &installation_id, "", "", "", "");
        }
        return PreparedRequest {
            headers,
            body: serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec()),
        };
    }

    let session_id = stable_id(&account.id, "session");
    let thread_id = if mode == CodexFingerprintMode::Full {
        session_id.clone()
    } else {
        stable_id(&format!("{}:{original_session}", account.id), "thread")
    };
    let turn_id = Uuid::new_v4().to_string();
    let window_id = format!("{thread_id}:0");

    set_header(&mut headers, "session_id", &session_id);
    set_header(&mut headers, "session-id", &session_id);
    set_header(&mut headers, "thread-id", &thread_id);
    set_header(&mut headers, "x-codex-window-id", &window_id);
    set_header(&mut headers, "x-client-request-id", &thread_id);
    rewrite_header_turn_metadata(
        &mut headers,
        &[
            ("installation_id", &installation_id),
            ("session_id", &session_id),
            ("thread_id", &thread_id),
            ("turn_id", &turn_id),
            ("window_id", &window_id),
        ],
    );

    if let Some(object) = object {
        rewrite_client_metadata(
            object,
            mode,
            &installation_id,
            &session_id,
            &thread_id,
            &turn_id,
            &window_id,
        );
    }
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec());
    PreparedRequest { headers, body }
}

fn first_non_empty<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn set_header(headers: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

fn rewrite_header_turn_metadata(headers: &mut HeaderMap, fields: &[(&str, &str)]) {
    let Some(raw) = header_text(headers, "x-codex-turn-metadata") else {
        return;
    };
    let Ok(mut metadata) = serde_json::from_str::<serde_json::Map<String, Value>>(&raw) else {
        return;
    };
    for (key, value) in fields {
        metadata.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    if fields.len() > 1 {
        metadata.insert(
            "turn_started_at_unix_ms".to_string(),
            Value::from(chrono::Utc::now().timestamp_millis()),
        );
    }
    if let Ok(encoded) = serde_json::to_string(&metadata) {
        set_header(headers, "x-codex-turn-metadata", &encoded);
    }
}

fn rewrite_client_metadata(
    body: &mut serde_json::Map<String, Value>,
    mode: CodexFingerprintMode,
    installation_id: &str,
    session_id: &str,
    thread_id: &str,
    turn_id: &str,
    window_id: &str,
) {
    let metadata = body
        .entry("client_metadata".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let Some(metadata) = metadata.as_object_mut() else {
        return;
    };
    metadata.insert(
        "x-codex-installation-id".to_string(),
        Value::String(installation_id.to_string()),
    );
    let mut embedded_fields = vec![("installation_id", installation_id)];
    if mode != CodexFingerprintMode::Device {
        for (key, value) in [
            ("session_id", session_id),
            ("thread_id", thread_id),
            ("turn_id", turn_id),
            ("x-codex-window-id", window_id),
        ] {
            metadata.insert(key.to_string(), Value::String(value.to_string()));
        }
        embedded_fields.extend([
            ("session_id", session_id),
            ("thread_id", thread_id),
            ("turn_id", turn_id),
            ("window_id", window_id),
        ]);
    }
    let Some(raw) = metadata
        .get("x-codex-turn-metadata")
        .and_then(Value::as_str)
    else {
        return;
    };
    let Ok(mut embedded) = serde_json::from_str::<serde_json::Map<String, Value>>(raw) else {
        return;
    };
    for (key, value) in embedded_fields {
        embedded.insert(key.to_string(), Value::String(value.to_string()));
    }
    if mode != CodexFingerprintMode::Device {
        embedded.insert(
            "turn_started_at_unix_ms".to_string(),
            Value::from(chrono::Utc::now().timestamp_millis()),
        );
    }
    if let Ok(encoded) = serde_json::to_string(&embedded) {
        metadata.insert("x-codex-turn-metadata".to_string(), Value::String(encoded));
    }
}

fn stable_id(seed: &str, scope: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"aether-codex-fingerprint:");
    digest.update(scope.as_bytes());
    digest.update(b":");
    digest.update(seed.as_bytes());
    let bytes = digest.finalize();
    let mut raw = [0_u8; 16];
    raw.copy_from_slice(&bytes[..16]);
    raw[6] = (raw[6] & 0x0f) | 0x40;
    raw[8] = (raw[8] & 0x3f) | 0x80;
    Uuid::from_bytes(raw).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewAccount;

    fn oauth_account() -> Account {
        let db = Db::new(std::path::Path::new(":memory:")).unwrap();
        db.upsert_account(&NewAccount {
            name: "fingerprint".to_string(),
            account_type: "oauth".to_string(),
            access_token: "token".to_string(),
            ..NewAccount::default()
        })
        .unwrap()
        .0
    }

    #[test]
    fn session_mode_keeps_headers_and_body_consistent() {
        let account = oauth_account();
        let mut headers = HeaderMap::new();
        headers.insert("session-id", HeaderValue::from_static("client-session"));
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(
                r#"{"installation_id":"old","session_id":"old","thread_id":"old","turn_id":"old","sandbox":"workspace-write"}"#,
            ),
        );
        let body = br#"{"client_metadata":{"x-codex-turn-metadata":"{\"turn_id\":\"old\",\"sandbox\":\"workspace-write\"}"}}"#;
        let prepared = prepare(&account, CodexFingerprintMode::Session, &headers, body);
        let header_metadata: Value = serde_json::from_str(
            prepared
                .headers
                .get("x-codex-turn-metadata")
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let body: Value = serde_json::from_slice(&prepared.body).unwrap();
        let metadata = body.get("client_metadata").unwrap();
        assert_eq!(metadata.get("turn_id"), header_metadata.get("turn_id"));
        assert_eq!(metadata.get("thread_id"), header_metadata.get("thread_id"));
        assert_eq!(header_metadata.get("sandbox").unwrap(), "workspace-write");
    }

    #[test]
    fn session_mode_isolates_threads_and_full_mode_converges_them() {
        let account = oauth_account();
        let prepare_for = |mode, session: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert("session-id", HeaderValue::from_static(session));
            prepare(&account, mode, &headers, br#"{}"#)
        };
        let first = prepare_for(CodexFingerprintMode::Session, "first");
        let second = prepare_for(CodexFingerprintMode::Session, "second");
        assert_ne!(
            first.headers.get("thread-id"),
            second.headers.get("thread-id")
        );
        assert_eq!(
            first.headers.get("session-id"),
            second.headers.get("session-id")
        );

        let first = prepare_for(CodexFingerprintMode::Full, "first");
        let second = prepare_for(CodexFingerprintMode::Full, "second");
        assert_eq!(
            first.headers.get("thread-id"),
            second.headers.get("thread-id")
        );
    }
}
