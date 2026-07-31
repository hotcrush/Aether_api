use super::AppState;
use crate::{codex_history, codex_takeover};

#[tauri::command]
pub(crate) fn get_codex_takeover_status(
    state: tauri::State<AppState>,
) -> Result<codex_takeover::CodexTakeoverStatus, String> {
    let proxy_base_url = format!("http://127.0.0.1:{}/v1", state.proxy_port);
    codex_takeover::takeover_status(&state.db, &proxy_base_url)
}

#[tauri::command]
pub(crate) fn set_codex_takeover(
    state: tauri::State<AppState>,
    enabled: bool,
) -> Result<codex_takeover::CodexTakeoverStatus, String> {
    let proxy_base_url = format!("http://127.0.0.1:{}/v1", state.proxy_port);
    if enabled {
        let access_token = state.access_token.load().as_str().to_owned();
        codex_takeover::enable_takeover(&state.db, &proxy_base_url, &access_token)
    } else {
        codex_takeover::disable_takeover(&state.db, &proxy_base_url)
    }
}

#[tauri::command]
pub(crate) fn get_codex_session_history_status(
    state: tauri::State<AppState>,
) -> Result<codex_history::CodexSessionHistoryStatus, String> {
    codex_history::session_history_status(&state.app_data_dir)
}

#[tauri::command]
pub(crate) fn has_codex_session_history_backup(
    state: tauri::State<AppState>,
) -> Result<bool, String> {
    codex_history::has_unify_history_backup(&state.app_data_dir)
}

#[tauri::command]
pub(crate) fn migrate_codex_session_history(
    state: tauri::State<AppState>,
) -> Result<codex_history::CodexSessionHistoryMigrationResult, String> {
    codex_history::migrate_existing_history(&state.app_data_dir)
}

#[tauri::command]
pub(crate) fn restore_codex_session_history(
    state: tauri::State<AppState>,
) -> Result<codex_history::CodexSessionHistoryRestoreResult, String> {
    codex_history::restore_official_history(&state.app_data_dir)
}
