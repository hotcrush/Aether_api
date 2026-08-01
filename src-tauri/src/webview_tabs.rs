use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{
    AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, Rect, Runtime, State,
    Webview, WebviewBuilder, WebviewUrl,
};

pub(crate) const ACTIVITY_EVENT: &str = "webview:activity";
pub(crate) const OPEN_REQUESTED_EVENT: &str = "webview:open-requested";

const MAIN_WEBVIEW_LABEL: &str = "main";
const MANAGED_WEBVIEW_PREFIX: &str = "web-";
const MAX_TAB_ID_LENGTH: usize = 256;
const MAX_LOGICAL_BOUND: f64 = 100_000.0;

#[derive(Clone)]
struct ManagedWebview {
    label: String,
    copy_proof: String,
}

#[derive(Default)]
pub(crate) struct WorkspaceWebviewState {
    by_tab_id: Mutex<HashMap<String, ManagedWebview>>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceWebviewBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateWorkspaceWebviewRequest {
    tab_id: String,
    url: String,
    bounds: WorkspaceWebviewBounds,
    #[serde(default = "default_visible")]
    visible: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActiveWorkspaceWebview {
    tab_id: String,
    url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceWebviewDescriptor {
    tab_id: String,
    webview_label: String,
    reused: bool,
    origin: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebviewActivity {
    id: String,
    tab_id: String,
    kind: &'static str,
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    success: Option<bool>,
    occurred_at: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NewTabRequested {
    id: String,
    source_tab_id: String,
    url: String,
    title: String,
    occurred_at: String,
}

fn default_visible() -> bool {
    true
}

fn registry(
    state: &WorkspaceWebviewState,
) -> Result<MutexGuard<'_, HashMap<String, ManagedWebview>>, String> {
    state
        .by_tab_id
        .lock()
        .map_err(|_| "WebView registry is unavailable".to_string())
}

fn validate_tab_id(tab_id: &str) -> Result<(), String> {
    if tab_id.is_empty() || tab_id.len() > MAX_TAB_ID_LENGTH || tab_id.chars().any(char::is_control)
    {
        return Err("Invalid workspace tab id".to_string());
    }
    Ok(())
}

impl WorkspaceWebviewBounds {
    fn validate(self) -> Result<Self, String> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|value| !value.is_finite())
            || self.x < 0.0
            || self.y < 0.0
            || self.width < 1.0
            || self.height < 1.0
            || values.iter().any(|value| *value > MAX_LOGICAL_BOUND)
        {
            return Err("Invalid WebView bounds".to_string());
        }
        Ok(self)
    }

    fn as_rect(self) -> Rect {
        Rect {
            position: LogicalPosition::new(self.x, self.y).into(),
            size: LogicalSize::new(self.width, self.height).into(),
        }
    }
}

fn parse_external_url(raw: &str) -> Result<tauri::Url, String> {
    let url = raw
        .parse::<tauri::Url>()
        .map_err(|_| "Invalid WebView URL".to_string())?;
    if !is_allowed_external_url(&url) {
        return Err(
            "Only http and https WebView URLs without embedded credentials are allowed".to_string(),
        );
    }
    Ok(url)
}

fn is_allowed_external_url(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn safe_origin(url: &tauri::Url) -> Option<String> {
    is_allowed_external_url(url).then(|| url.origin().ascii_serialization())
}

fn webview_origin<R: Runtime>(webview: &Webview<R>) -> Option<String> {
    webview.url().ok().and_then(|url| safe_origin(&url))
}

fn safe_file_name(path: Option<&Path>, url: &tauri::Url) -> Option<String> {
    let candidate = path
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .or_else(|| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .filter(|segment| !segment.is_empty())
        })?;
    let sanitized: String = candidate
        .chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\'))
        .take(255)
        .collect();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn activity(
    tab_id: String,
    kind: &'static str,
    phase: &'static str,
    origin: Option<String>,
    file_name: Option<String>,
    success: Option<bool>,
) -> WebviewActivity {
    WebviewActivity {
        id: uuid::Uuid::new_v4().to_string(),
        tab_id,
        kind,
        phase,
        origin,
        file_name,
        success,
        occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    }
}

fn emit_activity<R: Runtime>(webview: &Webview<R>, payload: WebviewActivity) {
    tracing::info!(
        tab_id = %payload.tab_id,
        kind = payload.kind,
        phase = payload.phase,
        origin = ?payload.origin,
        file_name = ?payload.file_name,
        success = ?payload.success,
        "WebView activity"
    );
    if let Err(error) = webview.emit_to(
        EventTarget::webview(MAIN_WEBVIEW_LABEL),
        ACTIVITY_EVENT,
        payload,
    ) {
        tracing::warn!(%error, "failed to emit WebView activity");
    }
}

fn emit_new_tab_requested<R: Runtime>(
    app: &AppHandle<R>,
    source_tab_id: &str,
    requested_url: &tauri::Url,
) {
    let Ok(url) = parse_external_url(requested_url.as_str()) else {
        return;
    };
    let payload = NewTabRequested {
        id: uuid::Uuid::new_v4().to_string(),
        source_tab_id: source_tab_id.to_string(),
        title: url
            .host_str()
            .expect("validated external URLs always have a host")
            .to_string(),
        url: url.to_string(),
        occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    if let Err(error) = app.emit_to(
        EventTarget::webview(MAIN_WEBVIEW_LABEL),
        OPEN_REQUESTED_EVENT,
        payload,
    ) {
        tracing::warn!(%error, "failed to emit WebView new-tab request");
    }
}

fn copy_initialization_script(proof: &str) -> String {
    let proof = serde_json::to_string(proof).expect("copy proof must serialize");
    format!(
        r#"
(() => {{
  const internals = window.__TAURI_INTERNALS__;
  if (internals && typeof internals.invoke === 'function') {{
    const invoke = internals.invoke.bind(internals);
    const proof = {proof};
    const reportCopy = () => {{
      setTimeout(() => {{
        void Promise.resolve(
          invoke('plugin:webview-audit|report_copy', {{ proof }})
        ).catch(() => undefined);
      }}, 0);
    }};
    const onCopy = (event) => {{
      if (!event.isTrusted) return;
      reportCopy();
    }};
    document.addEventListener('copy', onCopy, true);
    document.addEventListener('cut', onCopy, true);

    const clipboardPrototype = globalThis.Clipboard?.prototype;
    for (const method of ['write', 'writeText']) {{
      const original = clipboardPrototype?.[method];
      if (typeof original !== 'function') continue;
      try {{
        Object.defineProperty(clipboardPrototype, method, {{
          configurable: true,
          writable: true,
          value: function (...args) {{
            return Promise.resolve(original.apply(this, args)).then((value) => {{
              reportCopy();
              return value;
            }});
          }},
        }});
      }} catch {{}}
    }}
  }}
}})();
"#
    )
}

fn ensure_main_caller<R: Runtime>(caller: &Webview<R>) -> Result<(), String> {
    if caller.label() != MAIN_WEBVIEW_LABEL || caller.window().label() != MAIN_WEBVIEW_LABEL {
        return Err("Workspace WebView commands are restricted to the main UI".to_string());
    }
    Ok(())
}

fn managed_entry(
    state: &WorkspaceWebviewState,
    tab_id: &str,
) -> Result<Option<ManagedWebview>, String> {
    Ok(registry(state)?.get(tab_id).cloned())
}

fn managed_webview<R: Runtime>(
    app: &AppHandle<R>,
    state: &WorkspaceWebviewState,
    tab_id: &str,
) -> Result<Option<Webview<R>>, String> {
    validate_tab_id(tab_id)?;
    let Some(entry) = managed_entry(state, tab_id)? else {
        return Ok(None);
    };
    if let Some(webview) = app.get_webview(&entry.label) {
        return Ok(Some(webview));
    }
    registry(state)?.remove(tab_id);
    Ok(None)
}

fn show_exclusively<R: Runtime>(
    app: &AppHandle<R>,
    state: &WorkspaceWebviewState,
    target: &Webview<R>,
) -> Result<(), String> {
    let labels: Vec<String> = registry(state)?
        .values()
        .map(|entry| entry.label.clone())
        .collect();
    for label in labels {
        if label != target.label() {
            if let Some(webview) = app.get_webview(&label) {
                webview.hide().map_err(|error| error.to_string())?;
            }
        }
    }
    target.show().map_err(|error| error.to_string())?;
    target.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
async fn report_copy<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, WorkspaceWebviewState>,
    proof: String,
) -> Result<(), String> {
    if !webview.label().starts_with(MANAGED_WEBVIEW_PREFIX)
        || webview.window().label() != MAIN_WEBVIEW_LABEL
    {
        return Err("Copy reports are restricted to managed WebViews".to_string());
    }
    let tab_id = registry(&state)?
        .iter()
        .find_map(|(tab_id, entry)| {
            (entry.label == webview.label() && entry.copy_proof == proof).then(|| tab_id.clone())
        })
        .ok_or_else(|| "Invalid copy report proof".to_string())?;

    emit_activity(
        &webview,
        activity(
            tab_id,
            "copy",
            "occurred",
            webview_origin(&webview),
            None,
            None,
        ),
    );
    Ok(())
}

pub(crate) fn audit_plugin<R: Runtime>() -> TauriPlugin<R> {
    PluginBuilder::new("webview-audit")
        .invoke_handler(tauri::generate_handler![report_copy])
        .build()
}

#[tauri::command]
pub(crate) async fn create_workspace_webview<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    request: CreateWorkspaceWebviewRequest,
) -> Result<WorkspaceWebviewDescriptor, String> {
    ensure_main_caller(&caller)?;
    validate_tab_id(&request.tab_id)?;
    let bounds = request.bounds.validate()?;
    let url = parse_external_url(&request.url)?;
    let origin = safe_origin(&url).expect("validated external URLs always have an origin");

    if let Some(webview) = managed_webview(&app, &state, &request.tab_id)? {
        if webview.url().map_err(|error| error.to_string())? != url {
            webview
                .navigate(url.clone())
                .map_err(|error| error.to_string())?;
        }
        webview
            .set_bounds(bounds.as_rect())
            .map_err(|error| error.to_string())?;
        if request.visible {
            show_exclusively(&app, &state, &webview)?;
        } else {
            webview.hide().map_err(|error| error.to_string())?;
        }
        return Ok(WorkspaceWebviewDescriptor {
            tab_id: request.tab_id,
            webview_label: webview.label().to_string(),
            reused: true,
            origin,
        });
    }

    let webview_label = format!("{MANAGED_WEBVIEW_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let copy_proof = uuid::Uuid::new_v4().simple().to_string();
    let download_tab_id = request.tab_id.clone();
    let download_app = app.clone();
    let new_tab_app = app.clone();
    let new_tab_source_tab_id = request.tab_id.clone();
    let requested_download_paths = Arc::new(Mutex::new(HashMap::<String, PathBuf>::new()));
    let builder = WebviewBuilder::new(webview_label.clone(), WebviewUrl::External(url))
        .focused(false)
        .devtools(cfg!(debug_assertions))
        .initialization_script_for_all_frames(copy_initialization_script(&copy_proof))
        .on_navigation(is_allowed_external_url)
        .on_new_window(move |url, _| {
            emit_new_tab_requested(&new_tab_app, &new_tab_source_tab_id, &url);
            NewWindowResponse::Deny
        })
        .on_download(move |webview, event| {
            let origin = webview_origin(&webview);
            match event {
                DownloadEvent::Requested { url, destination } => {
                    if !destination.as_os_str().is_empty() {
                        requested_download_paths
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(url.as_str().to_string(), destination.clone());
                    }
                    emit_activity(
                        &webview,
                        activity(
                            download_tab_id.clone(),
                            "download",
                            "requested",
                            origin,
                            safe_file_name(Some(destination.as_path()), &url),
                            None,
                        ),
                    );
                }
                DownloadEvent::Finished { url, path, success } => {
                    let requested_path = requested_download_paths
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(url.as_str());
                    let completed_path = path.or(requested_path);
                    let file_name = safe_file_name(completed_path.as_deref(), &url);
                    emit_activity(
                        &webview,
                        activity(
                            download_tab_id.clone(),
                            "download",
                            "finished",
                            origin,
                            file_name.clone(),
                            Some(success),
                        ),
                    );
                    if success {
                        if let Some(path) = completed_path {
                            crate::app::inspect_downloaded_import(
                                download_app.clone(),
                                path,
                                file_name,
                            );
                        }
                    }
                }
                _ => return true,
            }
            true
        });

    let host = app
        .get_webview_window(MAIN_WEBVIEW_LABEL)
        .ok_or_else(|| "Main application window is unavailable".to_string())?;
    let webview = host
        .as_ref()
        .window()
        .add_child(
            builder,
            LogicalPosition::new(bounds.x, bounds.y),
            LogicalSize::new(bounds.width, bounds.height),
        )
        .map_err(|error| error.to_string())?;

    registry(&state)?.insert(
        request.tab_id.clone(),
        ManagedWebview {
            label: webview_label.clone(),
            copy_proof,
        },
    );
    if request.visible {
        show_exclusively(&app, &state, &webview)?;
    } else {
        webview.hide().map_err(|error| error.to_string())?;
    }

    Ok(WorkspaceWebviewDescriptor {
        tab_id: request.tab_id,
        webview_label,
        reused: false,
        origin,
    })
}

#[tauri::command]
pub(crate) async fn sync_webview_tabs<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    active: Option<ActiveWorkspaceWebview>,
    open_tab_ids: Vec<String>,
    bounds: Option<WorkspaceWebviewBounds>,
) -> Result<(), String> {
    ensure_main_caller(&caller)?;
    for tab_id in &open_tab_ids {
        validate_tab_id(tab_id)?;
    }
    let open_tab_ids: HashSet<String> = open_tab_ids.into_iter().collect();

    let stale_tab_ids: Vec<String> = registry(&state)?
        .keys()
        .filter(|tab_id| !open_tab_ids.contains(*tab_id))
        .cloned()
        .collect();
    for tab_id in stale_tab_ids {
        if let Some(webview) = managed_webview(&app, &state, &tab_id)? {
            webview.close().map_err(|error| error.to_string())?;
        }
        registry(&state)?.remove(&tab_id);
    }

    let Some(active) = active else {
        let labels: Vec<String> = registry(&state)?
            .values()
            .map(|entry| entry.label.clone())
            .collect();
        for label in labels {
            if let Some(webview) = app.get_webview(&label) {
                webview.hide().map_err(|error| error.to_string())?;
            }
        }
        return Ok(());
    };

    validate_tab_id(&active.tab_id)?;
    if !open_tab_ids.contains(&active.tab_id) {
        return Err("Active WebView tab is not in the open tab list".to_string());
    }
    let bounds = bounds
        .ok_or_else(|| "Active WebView bounds are required".to_string())?
        .validate()?;

    if let Some(webview) = managed_webview(&app, &state, &active.tab_id)? {
        webview
            .set_bounds(bounds.as_rect())
            .map_err(|error| error.to_string())?;
        show_exclusively(&app, &state, &webview)?;
        return Ok(());
    }

    create_workspace_webview(
        caller,
        app,
        state,
        CreateWorkspaceWebviewRequest {
            tab_id: active.tab_id,
            url: active.url,
            bounds,
            visible: true,
        },
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub(crate) fn show_workspace_webview<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    tab_id: String,
) -> Result<bool, String> {
    ensure_main_caller(&caller)?;
    let Some(webview) = managed_webview(&app, &state, &tab_id)? else {
        return Ok(false);
    };
    show_exclusively(&app, &state, &webview)?;
    Ok(true)
}

#[tauri::command]
pub(crate) fn hide_workspace_webview<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    tab_id: String,
) -> Result<bool, String> {
    ensure_main_caller(&caller)?;
    let Some(webview) = managed_webview(&app, &state, &tab_id)? else {
        return Ok(false);
    };
    webview.hide().map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
pub(crate) fn close_workspace_webview<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    tab_id: String,
) -> Result<bool, String> {
    ensure_main_caller(&caller)?;
    let Some(webview) = managed_webview(&app, &state, &tab_id)? else {
        return Ok(false);
    };
    webview.close().map_err(|error| error.to_string())?;
    registry(&state)?.remove(&tab_id);
    Ok(true)
}

#[tauri::command]
pub(crate) fn set_workspace_webview_bounds<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    tab_id: String,
    bounds: WorkspaceWebviewBounds,
) -> Result<bool, String> {
    ensure_main_caller(&caller)?;
    let bounds = bounds.validate()?;
    let Some(webview) = managed_webview(&app, &state, &tab_id)? else {
        return Ok(false);
    };
    webview
        .set_bounds(bounds.as_rect())
        .map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
pub(crate) fn navigate_workspace_webview<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    tab_id: String,
    url: String,
) -> Result<bool, String> {
    ensure_main_caller(&caller)?;
    let url = parse_external_url(&url)?;
    let Some(webview) = managed_webview(&app, &state, &tab_id)? else {
        return Ok(false);
    };
    webview.navigate(url).map_err(|error| error.to_string())?;
    Ok(true)
}
