use crate::db::{ChannelMonitorAccountSnapshot, ChannelMonitorEventSnapshot};
use crate::model_integrity::ModelIntegrityResult;
use crate::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize)]
pub(crate) struct ChannelMonitorEvent {
    id: i64,
    request_id: String,
    attempt_index: i64,
    status: String,
    http_status: Option<i64>,
    ttfb_ms: Option<i64>,
    duration_ms: Option<i64>,
    endpoint_family: String,
    model: String,
    source: String,
    message: String,
    estimated_cost: Option<f64>,
    created_at: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ChannelMonitorItem {
    account_id: String,
    name: String,
    account_type: String,
    account_status: String,
    models: Vec<String>,
    integrity: Option<ModelIntegrityResult>,
    latest_status: Option<String>,
    latest_checked_at: Option<String>,
    latest_ttfb_ms: Option<i64>,
    current_capacity: i64,
    concurrency: i64,
    availability_24h: Option<f64>,
    availability_7d: Option<f64>,
    avg_ttfb_24h_ms: Option<f64>,
    avg_ttfb_7d_ms: Option<f64>,
    attempts_24h: i64,
    attempts_7d: i64,
    failed_24h: i64,
    failed_7d: i64,
    estimated_cost_24h: Option<f64>,
    estimated_cost_7d: Option<f64>,
    timeline: Vec<ChannelMonitorEvent>,
    #[serde(skip)]
    available_24h: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct ChannelMonitorSnapshot {
    generated_at: i64,
    total_24h: i64,
    available_24h: i64,
    failed_24h: i64,
    availability_24h: Option<f64>,
    avg_ttfb_24h_ms: Option<f64>,
    active_channels: i64,
    abnormal_channels: i64,
    items: Vec<ChannelMonitorItem>,
}

#[tauri::command]
pub(crate) fn get_channel_monitor_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<ChannelMonitorSnapshot, String> {
    let snapshots = state
        .db
        .channel_monitor_snapshot()
        .map_err(|error| error.to_string())?;
    let models = state
        .db
        .list_accounts()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|account| (account.id, account.models))
        .collect::<HashMap<_, _>>();
    let integrity = state
        .db
        .latest_model_integrity_results()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|result| (result.account_id.clone(), result))
        .collect::<HashMap<_, _>>();
    let capacities = state.capacity.snapshot();
    let items = snapshots
        .into_iter()
        .map(|snapshot| {
            let current_capacity = capacities
                .get(&snapshot.account_id)
                .copied()
                .unwrap_or_default()
                .max(0);
            let account_models = models
                .get(&snapshot.account_id)
                .cloned()
                .unwrap_or_default();
            let account_integrity = integrity.get(&snapshot.account_id).cloned();
            ChannelMonitorItem::from_snapshot(
                snapshot,
                current_capacity,
                account_models,
                account_integrity,
            )
        })
        .collect::<Vec<_>>();

    Ok(summarize(items))
}

#[tauri::command]
pub(crate) async fn probe_channel(
    state: tauri::State<'_, AppState>,
    account_id: String,
) -> Result<String, String> {
    crate::test_account(state, account_id).await
}

impl ChannelMonitorItem {
    fn from_snapshot(
        snapshot: ChannelMonitorAccountSnapshot,
        current_capacity: i64,
        models: Vec<String>,
        integrity: Option<ModelIntegrityResult>,
    ) -> Self {
        Self {
            account_id: snapshot.account_id,
            name: snapshot.name,
            account_type: snapshot.account_type,
            account_status: snapshot.account_status,
            models,
            integrity,
            latest_status: snapshot.latest_status,
            latest_checked_at: snapshot.latest_checked_at,
            latest_ttfb_ms: snapshot.latest_ttfb_ms,
            current_capacity,
            concurrency: snapshot.concurrency,
            availability_24h: snapshot.availability_24h,
            availability_7d: snapshot.availability_7d,
            avg_ttfb_24h_ms: snapshot.avg_ttfb_24h_ms,
            avg_ttfb_7d_ms: snapshot.avg_ttfb_7d_ms,
            attempts_24h: snapshot.attempts_24h,
            attempts_7d: snapshot.attempts_7d,
            failed_24h: snapshot.failed_24h,
            failed_7d: snapshot.failed_7d,
            estimated_cost_24h: snapshot.estimated_cost_24h,
            estimated_cost_7d: snapshot.estimated_cost_7d,
            timeline: snapshot
                .timeline
                .into_iter()
                .map(ChannelMonitorEvent::from)
                .collect(),
            available_24h: snapshot.available_24h,
        }
    }
}

impl From<ChannelMonitorEventSnapshot> for ChannelMonitorEvent {
    fn from(event: ChannelMonitorEventSnapshot) -> Self {
        Self {
            id: event.id,
            request_id: event.request_id,
            attempt_index: event.attempt_index,
            status: event.status,
            http_status: event.http_status,
            ttfb_ms: event.ttfb_ms,
            duration_ms: event.duration_ms,
            endpoint_family: event.endpoint_family,
            model: event.model,
            source: event.source,
            message: event.message,
            estimated_cost: event.estimated_cost,
            created_at: event.created_at,
        }
    }
}

fn summarize(items: Vec<ChannelMonitorItem>) -> ChannelMonitorSnapshot {
    let active = items
        .iter()
        .filter(|item| item.account_status == "active")
        .collect::<Vec<_>>();
    let total_24h = active.iter().map(|item| item.attempts_24h).sum();
    let available_24h = active.iter().map(|item| item.available_24h).sum();
    let failed_24h = active.iter().map(|item| item.failed_24h).sum();
    let (ttfb_total, ttfb_weight) =
        active
            .iter()
            .fold((0.0, 0_i64), |acc, item| match item.avg_ttfb_24h_ms {
                Some(value) if item.attempts_24h > 0 => (
                    acc.0 + value * item.attempts_24h as f64,
                    acc.1 + item.attempts_24h,
                ),
                _ => acc,
            });

    ChannelMonitorSnapshot {
        generated_at: unix_time_millis(),
        total_24h,
        available_24h,
        failed_24h,
        availability_24h: (total_24h > 0).then(|| available_24h as f64 / total_24h as f64 * 100.0),
        avg_ttfb_24h_ms: (ttfb_weight > 0).then(|| ttfb_total / ttfb_weight as f64),
        active_channels: active.len() as i64,
        abnormal_channels: active
            .iter()
            .filter(|item| matches!(item.latest_status.as_deref(), Some("failed" | "error")))
            .count() as i64,
        items,
    }
}

fn unix_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
