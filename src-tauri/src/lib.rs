mod account_import;
mod app;
mod billing_sync;
mod capacity;
mod channel_monitor;
mod codex_history;
mod codex_identity;
mod codex_takeover;
mod cost_guard;
mod db;
mod dns;
mod image_generation;
mod logger;
mod market;
mod market_notification;
mod model_integrity;
mod oauth;
mod outbound_proxy;
mod pricing;
mod proxy;
mod quota;
mod quota_headers;
mod relay_usage;
mod webview_tabs;

pub(crate) use app::test_account;
pub use app::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    app::run();
}
