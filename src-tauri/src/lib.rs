mod account_import;
mod app;
mod capacity;
mod channel_monitor;
mod codex_history;
mod codex_takeover;
mod db;
mod dns;
mod logger;
mod oauth;
mod pricing;
mod proxy;
mod quota;
mod relay_usage;

pub(crate) use app::test_account;
pub use app::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    app::run();
}
