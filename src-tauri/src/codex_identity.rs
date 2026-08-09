use crate::db::Db;
use arc_swap::ArcSwap;
use reqwest::RequestBuilder;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

pub(crate) const DEFAULT_CODEX_VERSION: &str = "0.147.0";
const MIN_CODEX_VERSION: &str = "0.144.0";
const VERSION_SETTING: &str = "codex_client_version_synced";
const VERSION_SYNCED_AT_SETTING: &str = "codex_client_version_synced_at";
const AUTO_SYNC_SETTING: &str = "codex_version_auto_sync_enabled";
const VERSION_SYNC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const VERSION_SYNC_TIMEOUT: Duration = Duration::from_secs(30);
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const RECENT_RELEASES_URL: &str = "https://api.github.com/repos/openai/codex/releases?per_page=30";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CodexClientSettings {
    pub auto_sync_enabled: bool,
    pub effective_version: String,
    pub synced_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CodexClientSettingsUpdate {
    pub auto_sync_enabled: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub(crate) fn load_version(db: &Db) -> String {
    let stored = db
        .get_setting(VERSION_SETTING)
        .ok()
        .flatten()
        .and_then(|value| normalize_version(&value));
    stored
        .filter(|value| compare_versions(value, DEFAULT_CODEX_VERSION) != Ordering::Less)
        .unwrap_or_else(|| DEFAULT_CODEX_VERSION.to_string())
}

pub(crate) fn settings(db: &Db, version: &ArcSwap<String>) -> CodexClientSettings {
    CodexClientSettings {
        auto_sync_enabled: auto_sync_enabled(db),
        effective_version: version.load_full().as_ref().clone(),
        synced_at: db
            .get_setting(VERSION_SYNCED_AT_SETTING)
            .ok()
            .flatten()
            .and_then(|value| value.parse::<i64>().ok()),
    }
}

pub(crate) fn set_auto_sync(db: &Db, enabled: bool) -> Result<(), String> {
    db.set_setting(AUTO_SYNC_SETTING, if enabled { "true" } else { "false" })
        .map_err(|error| error.to_string())
}

pub(crate) fn current_version(version: &ArcSwap<String>) -> String {
    version.load_full().as_ref().clone()
}

pub(crate) fn user_agent(version: &str) -> String {
    format!("codex-tui/{version} (Windows 11; x86_64) WindowsTerminal (codex-tui; {version})")
}

pub(crate) fn apply_identity(request: RequestBuilder, version: &str) -> RequestBuilder {
    request
        .header("User-Agent", user_agent(version))
        .header("originator", "codex-tui")
        .header("version", version)
}

pub(crate) fn start_version_sync(
    db: Arc<Db>,
    client: Arc<ArcSwap<reqwest::Client>>,
    version: Arc<ArcSwap<String>>,
) {
    tauri::async_runtime::spawn(async move {
        if !synced_recently(&db) {
            if let Err(error) =
                sync_latest_version(&db, client.load_full().as_ref(), &version, false).await
            {
                warn!(%error, "Codex 客户端版本启动同步失败");
            }
        }
        loop {
            tokio::time::sleep(VERSION_SYNC_INTERVAL).await;
            if let Err(error) =
                sync_latest_version(&db, client.load_full().as_ref(), &version, false).await
            {
                warn!(%error, "Codex 客户端版本定时同步失败");
            }
        }
    });
}

pub(crate) async fn sync_latest_version(
    db: &Db,
    client: &reqwest::Client,
    version: &ArcSwap<String>,
    force: bool,
) -> Result<CodexClientSettings, String> {
    if !force && !auto_sync_enabled(db) {
        return Ok(settings(db, version));
    }

    let latest = fetch_latest_stable_version(client).await?;
    let current = current_version(version);
    if compare_versions(&latest, &current) == Ordering::Greater {
        db.set_setting(VERSION_SETTING, &latest)
            .map_err(|error| format!("保存 Codex 客户端版本失败: {error}"))?;
        version.store(Arc::new(latest.clone()));
        info!(previous = %current, version = %latest, "Codex 客户端版本已同步");
    }
    db.set_setting(
        VERSION_SYNCED_AT_SETTING,
        &chrono::Utc::now().timestamp().to_string(),
    )
    .map_err(|error| format!("保存 Codex 版本同步时间失败: {error}"))?;
    Ok(settings(db, version))
}

async fn fetch_latest_stable_version(client: &reqwest::Client) -> Result<String, String> {
    let latest = client
        .get(LATEST_RELEASE_URL)
        .timeout(VERSION_SYNC_TIMEOUT)
        .header("User-Agent", "Aether-Codex-Version-Sync")
        .send()
        .await
        .map_err(|error| format!("读取 Codex 最新版本失败: {error}"))?;
    if latest.status().is_success() {
        if let Ok(release) = latest.json::<GitHubRelease>().await {
            if let Some(version) = stable_release_version(&release) {
                return Ok(version);
            }
        }
    }

    let response = client
        .get(RECENT_RELEASES_URL)
        .timeout(VERSION_SYNC_TIMEOUT)
        .header("User-Agent", "Aether-Codex-Version-Sync")
        .send()
        .await
        .map_err(|error| format!("读取 Codex 版本列表失败: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("读取 Codex 版本列表失败 ({status})"));
    }
    response
        .json::<Vec<GitHubRelease>>()
        .await
        .map_err(|error| format!("解析 Codex 版本列表失败: {error}"))?
        .iter()
        .filter_map(stable_release_version)
        .max_by(|left, right| compare_versions(left, right))
        .ok_or_else(|| "没有找到可用的 Codex 稳定版本".to_string())
}

fn stable_release_version(release: &GitHubRelease) -> Option<String> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = release.tag_name.trim().strip_prefix("rust-v")?;
    if version.contains('-') {
        return None;
    }
    normalize_version(version)
        .filter(|version| compare_versions(version, MIN_CODEX_VERSION) != Ordering::Less)
}

fn normalize_version(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    let core = value.split_once('-').map_or(value, |(core, _)| core);
    let segments = core.split('.').collect::<Vec<_>>();
    if !(2..=4).contains(&segments.len())
        || segments
            .iter()
            .any(|segment| segment.is_empty() || !segment.bytes().all(|byte| byte.is_ascii_digit()))
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return None;
    }
    Some(value.to_string())
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let components = |value: &str| {
        value
            .split_once('-')
            .map_or(value, |(core, _)| core)
            .split('.')
            .map(|segment| segment.parse::<u64>().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    let left = components(left);
    let right = components(right);
    for index in 0..left.len().max(right.len()) {
        match left
            .get(index)
            .copied()
            .unwrap_or_default()
            .cmp(&right.get(index).copied().unwrap_or_default())
        {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    Ordering::Equal
}

fn auto_sync_enabled(db: &Db) -> bool {
    db.get_setting(AUTO_SYNC_SETTING)
        .ok()
        .flatten()
        .map(|value| value.trim() != "false")
        .unwrap_or(true)
}

fn synced_recently(db: &Db) -> bool {
    let Some(synced_at) = db
        .get_setting(VERSION_SYNCED_AT_SETTING)
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return false;
    };
    chrono::Utc::now().timestamp().saturating_sub(synced_at)
        < VERSION_SYNC_INTERVAL.as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_orders_codex_versions() {
        assert_eq!(normalize_version("0.147.0").as_deref(), Some("0.147.0"));
        assert_eq!(
            normalize_version("0.148.0-alpha.1").as_deref(),
            Some("0.148.0-alpha.1")
        );
        assert!(normalize_version("rust-v0.147.0").is_none());
        assert_eq!(compare_versions("0.147.0", "0.146.2"), Ordering::Greater);
    }

    #[test]
    fn builds_paired_codex_tui_identity() {
        assert_eq!(
            user_agent("0.147.0"),
            "codex-tui/0.147.0 (Windows 11; x86_64) WindowsTerminal (codex-tui; 0.147.0)"
        );
    }
}
