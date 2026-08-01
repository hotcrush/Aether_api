mod classifier;
mod database;
mod engine;
mod legacy;
mod types;

pub use engine::MarketState;
pub use types::MarketEvent;

use engine::{analytics_snapshot, upsert_shop};
use std::sync::Arc;
use tauri::State;
use types::{
    MarketAlertSettings, MarketAnalyticsSnapshot, MarketEvent as Event, MarketRefreshResult,
    MarketShopInput, MarketSnapshot,
};

#[tauri::command]
pub async fn get_market_snapshot(
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketSnapshot, String> {
    Ok(state.current_snapshot().await)
}

#[tauri::command]
pub async fn refresh_market(
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketRefreshResult, String> {
    state.refresh(true).await
}

#[tauri::command]
pub async fn get_market_analytics(
    range: String,
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketAnalyticsSnapshot, String> {
    analytics_snapshot(&state, &range).await
}

#[tauri::command]
pub async fn list_market_alerts(
    limit: Option<usize>,
    state: State<'_, Arc<MarketState>>,
) -> Result<Vec<Event>, String> {
    state
        .database()
        .events(None, limit.unwrap_or(200).clamp(1, 1_000))
}

#[tauri::command]
pub async fn mark_market_alerts_read(
    event_ids: Option<Vec<String>>,
    state: State<'_, Arc<MarketState>>,
) -> Result<u64, String> {
    let changed = state.database().mark_events_read(
        &event_ids.unwrap_or_default(),
        &chrono::Utc::now().to_rfc3339(),
    )?;
    state.reload_snapshot().await?;
    Ok(changed)
}

#[tauri::command]
pub async fn get_market_alert_settings(
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketAlertSettings, String> {
    state.database().alert_settings()
}

#[tauri::command]
pub async fn update_market_alert_settings(
    settings: MarketAlertSettings,
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketAlertSettings, String> {
    state.database().set_alert_settings(&settings)?;
    Ok(settings)
}

#[tauri::command]
pub async fn upsert_market_shop(
    input: MarketShopInput,
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketSnapshot, String> {
    upsert_shop(&state, input).await
}

#[tauri::command]
pub async fn set_market_shop_enabled(
    token: String,
    enabled: bool,
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketSnapshot, String> {
    state.database().set_shop_enabled(&token, enabled)?;
    state.reload_snapshot().await
}

#[tauri::command]
pub async fn delete_market_shop(
    token: String,
    state: State<'_, Arc<MarketState>>,
) -> Result<MarketSnapshot, String> {
    state.database().delete_shop(&token)?;
    state.reload_snapshot().await
}
