mod account_import;
mod app;
mod capacity;
mod channel_monitor;
mod codex_history;
mod codex_takeover;
mod db;
mod dns;
mod logger;
mod market;
mod market_notification;
mod oauth;
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
