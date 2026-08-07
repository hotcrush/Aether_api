use super::AppState;
use crate::cost_guard::{self, CostGuardSettings};
use crate::db::Db;
use crate::outbound_proxy::{self, OutboundProxySettings};
use crate::{codex_identity, codex_takeover, pricing};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::warn;

const DEVELOPMENT_PROXY_PORT: u16 = 19_090;
const PRODUCTION_PROXY_PORT: u16 = 9_090;

#[tauri::command]
pub(crate) fn get_cost_guard_settings(state: tauri::State<AppState>) -> CostGuardSettings {
    state.cost_guard.load().as_ref().clone()
}

#[tauri::command]
pub(crate) fn update_cost_guard_settings(
    state: tauri::State<AppState>,
    settings: CostGuardSettings,
) -> Result<CostGuardSettings, String> {
    let settings = cost_guard::save(&state.db, settings)?;
    state.cost_guard.store(Arc::new(settings.clone()));
    Ok(settings)
}

#[tauri::command]
pub(crate) fn get_outbound_proxy_settings(state: tauri::State<AppState>) -> OutboundProxySettings {
    state.outbound_proxy.load().as_ref().clone()
}

#[tauri::command]
pub(crate) fn update_outbound_proxy_settings(
    state: tauri::State<AppState>,
    settings: OutboundProxySettings,
) -> Result<OutboundProxySettings, String> {
    let settings = settings.validate()?;
    let client = outbound_proxy::build_client(120, 15, &settings)?;
    let proxy_client = outbound_proxy::build_client(600, 15, &settings)?;
    let settings = outbound_proxy::save(&state.db, settings)?;
    state.client.store(Arc::new(client));
    state.proxy_client.store(Arc::new(proxy_client));
    state.outbound_proxy.store(Arc::new(settings.clone()));
    Ok(settings)
}

#[tauri::command]
pub(crate) fn get_codex_client_settings(
    state: tauri::State<AppState>,
) -> codex_identity::CodexClientSettings {
    codex_identity::settings(&state.db, &state.codex_version)
}

#[tauri::command]
pub(crate) async fn update_codex_client_settings(
    state: tauri::State<'_, AppState>,
    settings: codex_identity::CodexClientSettingsUpdate,
) -> Result<codex_identity::CodexClientSettings, String> {
    codex_identity::set_auto_sync(&state.db, settings.auto_sync_enabled)?;
    if settings.auto_sync_enabled {
        let client = state.client.load_full();
        if let Err(error) =
            codex_identity::sync_latest_version(&state.db, &client, &state.codex_version, false)
                .await
        {
            warn!(%error, "开启后立即同步 Codex 客户端版本失败");
        }
    }
    Ok(codex_identity::settings(&state.db, &state.codex_version))
}

#[tauri::command]
pub(crate) async fn sync_codex_client_version(
    state: tauri::State<'_, AppState>,
) -> Result<codex_identity::CodexClientSettings, String> {
    let client = state.client.load_full();
    codex_identity::sync_latest_version(&state.db, &client, &state.codex_version, true).await
}

#[tauri::command]
pub(crate) fn get_cache(
    state: tauri::State<AppState>,
    key: String,
) -> Result<Option<String>, String> {
    state
        .db
        .get_setting(&key)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn set_cache(
    state: tauri::State<AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    state
        .db
        .set_setting(&key, &value)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn merge_cache_entries(
    state: tauri::State<AppState>,
    key: String,
    entries: BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    let entries = entries
        .into_iter()
        .map(|(entry_key, value)| {
            serde_json::to_string(&value)
                .map(|encoded| (entry_key, encoded))
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    state
        .db
        .merge_json_setting_entries(&key, &entries)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_proxy_info(state: tauri::State<AppState>) -> serde_json::Value {
    let accounts = state.db.list_accounts().unwrap_or_default();
    let account_count = accounts.len();
    let active_account_count = accounts
        .iter()
        .filter(|account| account.status == "active")
        .count();
    let total_requests = state.db.total_request_count().unwrap_or(0);
    let usage = state.db.usage_totals().unwrap_or_default();
    let today_cost = state.db.today_estimated_cost().unwrap_or(0.0);
    let access_token = state.access_token.load().as_str().to_owned();
    json!({
        "port": state.proxy_port,
        "proxy_profile": state.proxy_profile,
        "base_url": format!("http://127.0.0.1:{}", state.proxy_port),
        "access_token": access_token,
        "running": state.proxy_running.load(Ordering::Acquire),
        "account_count": account_count,
        "active_account_count": active_account_count,
        "total_requests": total_requests,
        "total_tokens": usage.total_tokens,
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cached_tokens": usage.cached_tokens,
        "cache_write_tokens": usage.cache_write_tokens,
        "reasoning_tokens": usage.reasoning_tokens,
        "unpriced_tokens": usage.unpriced_tokens,
        "total_cost": usage.total_cost,
        "today_cost": today_cost,
        "pricing_updated_at": pricing::PRICING_UPDATED_AT,
        "pricing_source": pricing::PRICING_SOURCE,
        "account_capacities": state.capacity.snapshot(),
    })
}

pub(super) fn proxy_profile() -> (&'static str, &'static str, u16) {
    if cfg!(debug_assertions) {
        (
            "development",
            "proxy_port_development",
            DEVELOPMENT_PROXY_PORT,
        )
    } else {
        ("production", "proxy_port", PRODUCTION_PROXY_PORT)
    }
}

fn valid_proxy_port(value: &str) -> Option<u16> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port >= 1024)
}

pub(super) fn configured_proxy_port(db: &Db) -> u16 {
    let (profile, setting_key, default_port) = proxy_profile();
    let profile_env = if profile == "development" {
        "AETHER_DEVELOPMENT_PROXY_PORT"
    } else {
        "AETHER_PRODUCTION_PROXY_PORT"
    };
    for key in [profile_env, "AETHER_PROXY_PORT"] {
        if let Ok(value) = std::env::var(key) {
            if let Some(port) = valid_proxy_port(&value) {
                return port;
            }
            warn!(environment = key, value, "忽略无效的代理端口环境变量");
        }
    }
    let stored = db
        .get_or_create_setting(setting_key, &default_port.to_string())
        .ok();
    if let Some(port) = stored.as_deref().and_then(valid_proxy_port) {
        return port;
    }
    let _ = db.set_setting(setting_key, &default_port.to_string());
    default_port
}

#[tauri::command]
pub(crate) fn reset_access_token(state: tauri::State<AppState>) -> Result<String, String> {
    let access_token = format!("sk-local-{}", uuid::Uuid::new_v4().simple());
    let proxy_base_url = format!("http://127.0.0.1:{}/v1", state.proxy_port);
    codex_takeover::refresh_takeover_token_if_active(&state.db, &proxy_base_url, &access_token)?;
    state
        .db
        .set_setting("access_token", &access_token)
        .map_err(|error| error.to_string())?;
    state.access_token.store(Arc::new(access_token.clone()));
    Ok(access_token)
}

/// Build-time metadata injected by build.rs.
const GIT_COMMIT: &str = env!("AETHER_GIT_COMMIT");
const BUILD_TIME: &str = env!("AETHER_BUILD_TIME");

#[tauri::command]
pub(crate) fn get_app_version() -> serde_json::Value {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commit": GIT_COMMIT,
        "build_time": BUILD_TIME,
        "profile": profile,
        "tauri_version": tauri::VERSION,
    })
}
