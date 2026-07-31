use super::AppState;
use crate::db;

#[tauri::command]
pub(crate) fn list_request_logs(
    state: tauri::State<'_, AppState>,
    query: db::RequestLogQuery,
) -> Result<db::RequestLogPage, String> {
    state
        .db
        .list_request_logs(query)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn clear_request_logs(state: tauri::State<'_, AppState>) -> Result<u64, String> {
    state
        .db
        .clear_request_logs()
        .map_err(|error| error.to_string())
}
