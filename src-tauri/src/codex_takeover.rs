use crate::db::Db;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use toml_edit::{value, DocumentMut, Item};

const BACKUP_SETTING_KEY: &str = "codex_takeover_backup_v1";
const AETHER_PROVIDER_ID: &str = crate::codex_history::UNIFIED_CODEX_PROVIDER_ID;
const AETHER_PROVIDER_NAME: &str = "Aether Local";
const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";

#[derive(Debug, Clone, Serialize)]
pub struct CodexTakeoverStatus {
    pub active: bool,
    pub backup_available: bool,
    pub codex_dir: String,
    pub auth_path: String,
    pub config_path: String,
    pub expected_base_url: String,
    pub configured_base_url: Option<String>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileSnapshot {
    existed: bool,
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexTakeoverBackup {
    auth: FileSnapshot,
    config: FileSnapshot,
    created_at: String,
}

struct CodexPaths {
    codex_dir: PathBuf,
    auth_path: PathBuf,
    config_path: PathBuf,
}

pub fn takeover_status(db: &Db, proxy_base_url: &str) -> Result<CodexTakeoverStatus, String> {
    let paths = codex_paths()?;
    let config = read_optional(&paths.config_path)?;
    let active_provider = config.as_deref().and_then(active_provider_config);
    let provider_id = active_provider.as_ref().map(|provider| provider.0.clone());
    let configured_base_url = active_provider.and_then(|provider| provider.1);
    let model = config.as_deref().and_then(active_model);
    let expected_base_url = normalize_url(proxy_base_url);
    let active = provider_id.as_deref() == Some(AETHER_PROVIDER_ID)
        && configured_base_url
            .as_deref()
            .map(normalize_url)
            .is_some_and(|base_url| base_url == expected_base_url);
    Ok(CodexTakeoverStatus {
        active,
        backup_available: db
            .get_setting(BACKUP_SETTING_KEY)
            .map_err(|error| error.to_string())?
            .is_some(),
        codex_dir: paths.codex_dir.display().to_string(),
        auth_path: paths.auth_path.display().to_string(),
        config_path: paths.config_path.display().to_string(),
        expected_base_url,
        configured_base_url,
        provider_id,
        model,
    })
}

pub fn enable_takeover(
    db: &Db,
    proxy_base_url: &str,
    access_token: &str,
) -> Result<CodexTakeoverStatus, String> {
    let paths = codex_paths()?;
    let current_config = read_optional(&paths.config_path)?;
    let expected_base_url = normalize_url(proxy_base_url);
    let already_active = current_config
        .as_deref()
        .and_then(active_provider_config)
        .filter(|provider| provider.0 == AETHER_PROVIDER_ID)
        .and_then(|provider| provider.1)
        .map(|url| normalize_url(&url))
        .is_some_and(|base_url| base_url == expected_base_url);

    if !already_active
        && db
            .get_setting(BACKUP_SETTING_KEY)
            .map_err(|error| error.to_string())?
            .is_none()
    {
        let backup = CodexTakeoverBackup {
            auth: snapshot(&paths.auth_path)?,
            config: snapshot(&paths.config_path)?,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let backup_json = serde_json::to_string(&backup)
            .map_err(|error| format!("序列化 Codex 备份失败: {error}"))?;
        db.set_setting(BACKUP_SETTING_KEY, &backup_json)
            .map_err(|error| format!("保存 Codex 备份失败: {error}"))?;
    }

    let updated_config = apply_takeover_config(
        current_config.as_deref().unwrap_or(""),
        &expected_base_url,
        access_token,
    )?;
    write_text(&paths.config_path, &updated_config)?;
    takeover_status(db, &expected_base_url)
}

pub fn disable_takeover(db: &Db, proxy_base_url: &str) -> Result<CodexTakeoverStatus, String> {
    let paths = codex_paths()?;
    let backup_json = db
        .get_setting(BACKUP_SETTING_KEY)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "没有可恢复的 Codex 接管备份".to_string())?;
    let backup: CodexTakeoverBackup = serde_json::from_str(&backup_json)
        .map_err(|error| format!("读取 Codex 接管备份失败: {error}"))?;

    restore_snapshot(&paths.auth_path, &backup.auth)?;
    restore_snapshot(&paths.config_path, &backup.config)?;
    db.delete_setting(BACKUP_SETTING_KEY)
        .map_err(|error| format!("清理 Codex 接管备份失败: {error}"))?;
    takeover_status(db, proxy_base_url)
}

pub fn refresh_takeover_token_if_active(
    db: &Db,
    proxy_base_url: &str,
    access_token: &str,
) -> Result<(), String> {
    let status = takeover_status(db, proxy_base_url)?;
    if status.provider_id.as_deref() != Some(AETHER_PROVIDER_ID) {
        return Ok(());
    }

    let paths = codex_paths()?;
    let current_config = read_optional(&paths.config_path)?.unwrap_or_default();
    let updated_config = apply_takeover_config(
        &current_config,
        &normalize_url(proxy_base_url),
        access_token,
    )?;
    write_text(&paths.config_path, &updated_config)
}

fn codex_paths() -> Result<CodexPaths, String> {
    let home = home_dir().ok_or_else(|| "无法定位用户主目录，不能写入 Codex 配置".to_string())?;
    let codex_dir = home.join(".codex");
    Ok(CodexPaths {
        auth_path: codex_dir.join("auth.json"),
        config_path: codex_dir.join("config.toml"),
        codex_dir,
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn snapshot(path: &PathBuf) -> Result<FileSnapshot, String> {
    if path.exists() {
        Ok(FileSnapshot {
            existed: true,
            content: std::fs::read_to_string(path)
                .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?,
        })
    } else {
        Ok(FileSnapshot {
            existed: false,
            content: String::new(),
        })
    }
}

fn read_optional(path: &PathBuf) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))
}

fn write_text(path: &PathBuf, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败: {error}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|error| format!("写入 {} 失败: {error}", path.display()))
}

fn restore_snapshot(path: &PathBuf, snapshot: &FileSnapshot) -> Result<(), String> {
    if snapshot.existed {
        write_text(path, &snapshot.content)
    } else if path.exists() {
        std::fs::remove_file(path).map_err(|error| format!("删除 {} 失败: {error}", path.display()))
    } else {
        Ok(())
    }
}

fn parse_document(config_text: &str) -> Result<DocumentMut, String> {
    if config_text.trim().is_empty() {
        Ok(DocumentMut::new())
    } else {
        config_text
            .parse::<DocumentMut>()
            .map_err(|error| format!("Codex config.toml 解析失败: {error}"))
    }
}

fn apply_takeover_config(
    config_text: &str,
    proxy_base_url: &str,
    access_token: &str,
) -> Result<String, String> {
    let mut doc = parse_document(config_text)?;
    let current_model = active_model_from_doc(&doc);

    doc["model_provider"] = value(AETHER_PROVIDER_ID);
    if current_model.is_none() {
        doc["model"] = value(DEFAULT_CODEX_MODEL);
    }
    if doc.get("model_reasoning_effort").is_none() {
        doc["model_reasoning_effort"] = value("high");
    }
    if doc.get("disable_response_storage").is_none() {
        doc["disable_response_storage"] = value(true);
    }

    ensure_table(&mut doc, "model_providers");
    let model_providers = doc["model_providers"]
        .as_table_like_mut()
        .ok_or_else(|| "Codex model_providers 不是有效表".to_string())?;
    if model_providers
        .get(AETHER_PROVIDER_ID)
        .is_none_or(|item| item.as_table_like().is_none())
    {
        model_providers.insert(AETHER_PROVIDER_ID, Item::Table(toml_edit::Table::new()));
    }
    let provider = model_providers
        .get_mut(AETHER_PROVIDER_ID)
        .and_then(|item| item.as_table_like_mut())
        .ok_or_else(|| "Codex Aether provider 不是有效表".to_string())?;
    provider.insert("name", value(AETHER_PROVIDER_NAME));
    provider.insert("base_url", value(normalize_url(proxy_base_url)));
    provider.insert("wire_api", value("responses"));
    provider.insert("supports_websockets", value(true));
    provider.insert("requires_openai_auth", value(true));
    provider.insert("experimental_bearer_token", value(access_token.trim()));

    Ok(doc.to_string())
}

fn ensure_table(doc: &mut DocumentMut, key: &str) {
    if doc
        .get(key)
        .is_none_or(|item| item.as_table_like().is_none())
    {
        doc[key] = toml_edit::table();
    }
}

fn active_model(config_text: &str) -> Option<String> {
    parse_document(config_text)
        .ok()
        .and_then(|doc| active_model_from_doc(&doc))
}

fn active_model_from_doc(doc: &DocumentMut) -> Option<String> {
    doc.get("model")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn active_provider_config(config_text: &str) -> Option<(String, Option<String>)> {
    let doc = parse_document(config_text).ok()?;
    let active_provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)?
        .to_string();
    let base_url = doc
        .get("model_providers")
        .and_then(|item| item.as_table_like())
        .and_then(|providers| providers.get(&active_provider))
        .and_then(|item| item.as_table_like())
        .and_then(|provider| provider.get("base_url"))
        .and_then(|item| item.as_str())
        .map(normalize_url);
    Some((active_provider, base_url))
}

fn normalize_url(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takeover_preserves_unrelated_codex_config() {
        let input = r#"approval_policy = "never"

[mcp_servers.fetch]
command = "uvx"
args = ["mcp-server-fetch"]
"#;

        let output = apply_takeover_config(input, "http://127.0.0.1:9090/v1/", "sk-local")
            .expect("apply takeover config");
        let parsed = output.parse::<DocumentMut>().expect("parse output");

        assert_eq!(
            parsed["mcp_servers"]["fetch"]["command"].as_str(),
            Some("uvx")
        );
        assert_eq!(parsed["approval_policy"].as_str(), Some("never"));
        assert_eq!(parsed["model_provider"].as_str(), Some(AETHER_PROVIDER_ID));
        assert_eq!(
            parsed["model_providers"][AETHER_PROVIDER_ID]["base_url"].as_str(),
            Some("http://127.0.0.1:9090/v1")
        );
        assert_eq!(
            parsed["model_providers"][AETHER_PROVIDER_ID]["experimental_bearer_token"].as_str(),
            Some("sk-local")
        );
        assert_eq!(
            parsed["model_providers"][AETHER_PROVIDER_ID]["supports_websockets"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn takeover_overwrites_legacy_websocket_settings() {
        let input = r#"model_provider = "aether"

[model_providers.aether]
base_url = "http://127.0.0.1:8080/v1"
supports_websockets = false
experimental_bearer_token = "old-token"
"#;

        let output = apply_takeover_config(input, "http://127.0.0.1:9090/v1", "new-token")
            .expect("upgrade takeover config");
        let parsed = output.parse::<DocumentMut>().expect("parse output");
        let provider = &parsed["model_providers"][AETHER_PROVIDER_ID];

        assert_eq!(
            provider["base_url"].as_str(),
            Some("http://127.0.0.1:9090/v1")
        );
        assert_eq!(provider["supports_websockets"].as_bool(), Some(true));
        assert_eq!(
            provider["experimental_bearer_token"].as_str(),
            Some("new-token")
        );
    }
}
