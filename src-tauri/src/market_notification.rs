use crate::market::MarketEvent;
use notify_rust::{Notification, NotificationResponse};
use tauri::{AppHandle, Emitter, Manager};

pub fn show_market_notification(app: &AppHandle, event: &MarketEvent) -> Result<(), String> {
    let mut notification = Notification::new();
    notification
        .summary(&event.title)
        .body(&event.body)
        .auto_icon()
        .action("open", "打开 Aether");

    #[cfg(windows)]
    notification.app_id(&app.config().identifier);

    let handle = notification.show().map_err(|error| error.to_string())?;
    let app = app.clone();
    let event = event.clone();
    std::thread::spawn(move || {
        let _ = handle.wait_for_response(move |response: &NotificationResponse| {
            if !matches!(
                response,
                NotificationResponse::Default | NotificationResponse::Action(_)
            ) {
                return;
            }
            if let Some(window) = app.get_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            let _ = app.emit("market:notification-opened", &event);
        });
    });
    Ok(())
}
