use super::clipboard::ClipboardImportState;
use super::proxy_settings::{configured_proxy_port, proxy_profile};
use super::vault::VaultState;
use super::AppState;
use crate::capacity::CapacityRegistry;
use crate::db::Db;
use crate::market::MarketState;
use crate::{dns, proxy};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

pub(crate) fn run() {
    let _guard =
        single_instance::SingleInstance::new("aether-sub2api-single").expect("无法创建单实例锁");
    if !_guard.is_single() {
        eprintln!("Aether 已在运行中");
        std::process::exit(0);
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tauri::Builder::default()
        .plugin(crate::webview_tabs::audit_plugin())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_stronghold::Builder::new(|password| {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut key = password.as_bytes().to_vec();
                for _ in 0..10_000 {
                    let mut hasher = DefaultHasher::new();
                    key.hash(&mut hasher);
                    let h = hasher.finish();
                    key = h.to_le_bytes().to_vec();
                }
                key
            })
            .build(),
        )
        .setup(|app| {
            let app_dir = app.path().app_data_dir().expect("无法获取应用数据目录");
            std::fs::create_dir_all(&app_dir).ok();

            let db = Arc::new(Db::new(&app_dir.join("sub2api.db")).expect("初始化数据库失败"));
            let proxy_port = configured_proxy_port(&db);
            let (proxy_profile, _, _) = proxy_profile();
            let generated_token = format!("sk-local-{}", uuid::Uuid::new_v4().simple());
            let access_token = db
                .get_or_create_setting("access_token", &generated_token)
                .expect("初始化本地访问密钥失败");
            let access_token = Arc::new(arc_swap::ArcSwap::new(Arc::new(access_token)));
            let proxy_running = Arc::new(AtomicBool::new(false));
            let capacity = Arc::new(CapacityRegistry::default());
            let client = dns::build_client(120, 15);
            let legacy_market_dir = std::env::var_os("CODEX_RELAY_DATA_DIR")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    app.path()
                        .document_dir()
                        .ok()
                        .map(|path| path.join("GitHub/codex-relay/server/data"))
                });
            let market = MarketState::new(
                app_dir.join("market.db"),
                client.clone(),
                app.handle().clone(),
                legacy_market_dir.as_deref(),
            )
            .map_err(std::io::Error::other)?;

            let proxy_db = Arc::clone(&db);
            let proxy_token = Arc::clone(&access_token);
            let proxy_status = Arc::clone(&proxy_running);
            let proxy_capacity = Arc::clone(&capacity);
            let proxy_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                proxy::start_proxy_server(
                    proxy_db,
                    proxy_port,
                    proxy_token,
                    proxy_status,
                    proxy_capacity,
                    proxy_app,
                )
                .await;
            });

            app.manage(AppState {
                db,
                app_data_dir: app_dir.clone(),
                client,
                proxy_port,
                proxy_profile,
                capacity,
                access_token,
                proxy_running,
                clipboard_import: Mutex::new(ClipboardImportState::default()),
                clipboard_reading: AtomicBool::new(false),
            });
            app.manage(VaultState::new(app_dir.join("vault.stronghold")));
            app.manage(crate::webview_tabs::WorkspaceWebviewState::default());
            app.manage(Arc::clone(&market));
            MarketState::start(market);

            let show_item = MenuItemBuilder::with_id("show", "显示窗口").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "退出 Aether").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&show_item)
                .item(&quit_item)
                .build()?;

            let _tray = TrayIconBuilder::new()
                .tooltip("Aether")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            super::accounts::list_accounts,
            super::accounts::delete_account,
            super::accounts::list_trashed_accounts,
            super::accounts::restore_account,
            super::accounts::purge_account,
            super::accounts::purge_all_trashed,
            super::proxy_settings::get_cache,
            super::proxy_settings::set_cache,
            super::proxy_settings::merge_cache_entries,
            super::accounts::set_account_status,
            super::accounts::set_account_priority,
            super::accounts::set_account_concurrency,
            super::proxy_settings::get_proxy_info,
            super::clipboard::inspect_clipboard_import,
            super::clipboard::confirm_clipboard_import,
            super::clipboard::discard_clipboard_import,
            super::clipboard::import_accounts,
            super::accounts::refresh_account,
            super::accounts::refresh_all_accounts,
            super::logs::list_request_logs,
            super::logs::clear_request_logs,
            crate::channel_monitor::get_channel_monitor_snapshot,
            crate::channel_monitor::probe_channel,
            crate::market::get_market_snapshot,
            crate::market::refresh_market,
            crate::market::get_market_analytics,
            crate::market::list_market_alerts,
            crate::market::mark_market_alerts_read,
            crate::market::get_market_alert_settings,
            crate::market::update_market_alert_settings,
            crate::market::upsert_market_shop,
            crate::market::set_market_shop_enabled,
            crate::market::delete_market_shop,
            crate::webview_tabs::sync_webview_tabs,
            crate::webview_tabs::create_workspace_webview,
            crate::webview_tabs::show_workspace_webview,
            crate::webview_tabs::hide_workspace_webview,
            crate::webview_tabs::close_workspace_webview,
            crate::webview_tabs::set_workspace_webview_bounds,
            crate::webview_tabs::navigate_workspace_webview,
            super::accounts::test_account,
            super::usage::query_account_quota,
            super::usage::query_all_quotas,
            super::usage::query_relay_usage,
            super::usage::open_relay_site,
            super::accounts::export_accounts,
            super::accounts::reset_request_counts,
            super::proxy_settings::reset_access_token,
            super::proxy_settings::get_app_version,
            super::codex::get_codex_takeover_status,
            super::codex::set_codex_takeover,
            super::codex::get_codex_session_history_status,
            super::codex::has_codex_session_history_backup,
            super::codex::migrate_codex_session_history,
            super::codex::restore_codex_session_history,
            super::vault::init_vault,
            super::vault::lock_vault,
            super::vault::is_vault_unlocked,
            super::vault::store_secret,
            super::vault::get_secret,
            super::vault::delete_secret,
            super::vault::migrate_secrets_to_vault,
            super::vault::export_vault_secrets,
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 应用失败");
}
