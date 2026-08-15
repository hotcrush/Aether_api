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
struct DownloadTracker {
    by_url: HashMap<String, Vec<PathBuf>>,
    reserved: HashSet<PathBuf>,
}

impl DownloadTracker {
    fn reserve(&mut self, url: &tauri::Url, directory: &Path, file_name: &str) -> PathBuf {
        let path = unique_download_path(directory, file_name, &self.reserved);
        self.reserved.insert(path.clone());
        self.by_url
            .entry(url.as_str().to_string())
            .or_default()
            .push(path.clone());
        path
    }

    fn finish(&mut self, url: &tauri::Url, completed: Option<&Path>) -> Option<PathBuf> {
        let url_key = url.as_str().to_string();
        let (requested, remove_url) = match self.by_url.get_mut(&url_key) {
            Some(paths) => {
                let index = completed
                    .and_then(|completed| paths.iter().position(|path| path == completed))
                    .unwrap_or_default();
                let requested = (!paths.is_empty()).then(|| paths.remove(index));
                (requested, paths.is_empty())
            }
            None => (None, false),
        };
        if remove_url {
            self.by_url.remove(&url_key);
        }
        if let Some(path) = requested.as_ref() {
            self.reserved.remove(path);
        }
        completed.map(Path::to_path_buf).or(requested)
    }
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
    #[serde(default)]
    use_outbound_proxy: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActiveWorkspaceWebview {
    tab_id: String,
    url: String,
    #[serde(default)]
    use_outbound_proxy: bool,
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

fn needs_ldxp_request_header(url: &tauri::Url) -> bool {
    matches!(url.host_str(), Some("pay.ldxp.cn" | "www.ldxp.cn"))
}

#[cfg(windows)]
fn install_ldxp_request_header<R: Runtime>(webview: &Webview<R>) -> Result<(), String> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        WebResourceRequestedEventHandler,
    };
    use windows::core::{w, HSTRING};

    let label = webview.label().to_string();
    webview
        .with_webview(move |platform| {
            let result: webview2_com::Result<()> = (|| unsafe {
                let core = platform.controller().CoreWebView2()?;
                for pattern in ["https://pay.ldxp.cn/*", "https://www.ldxp.cn/*"] {
                    core.AddWebResourceRequestedFilter(
                        &HSTRING::from(pattern),
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    )?;
                }
                let handler = WebResourceRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.Request()?
                            .Headers()?
                            .SetHeader(w!("X-Real-IP"), w!("127.0.0.1"))?;
                    }
                    Ok(())
                }));
                let mut token = 0;
                core.add_WebResourceRequested(&handler, &mut token)?;
                Ok(())
            })();
            if let Err(error) = result {
                tracing::warn!(webview = %label, %error, "安装 ldxp WebView 请求头兼容层失败");
            }
        })
        .map_err(|error| error.to_string())
}

#[cfg(not(windows))]
fn install_ldxp_request_header<R: Runtime>(_webview: &Webview<R>) -> Result<(), String> {
    Ok(())
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
    sanitize_download_file_name(candidate)
}

fn sanitize_download_file_name(candidate: &str) -> Option<String> {
    let sanitized: String = candidate
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        .take(180)
        .collect();
    let sanitized = sanitized.trim().trim_end_matches(['.', ' ']);
    if sanitized.is_empty() || matches!(sanitized, "." | "..") {
        return None;
    }
    let stem = Path::new(sanitized)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit() && suffix != "0"
            });
    Some(if reserved {
        format!("_{sanitized}")
    } else {
        sanitized.to_string()
    })
}

fn workspace_download_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Ok(path) = app.path().download_dir() {
        candidates.push(path);
    }
    if let Ok(path) = app.path().app_data_dir() {
        candidates.push(path.join("downloads"));
    }
    for directory in candidates {
        if directory.is_absolute() && std::fs::create_dir_all(&directory).is_ok() {
            return Ok(directory);
        }
    }
    Err("无法创建 WebView 下载目录".to_string())
}

fn unique_download_path(directory: &Path, file_name: &str, reserved: &HashSet<PathBuf>) -> PathBuf {
    let initial = directory.join(file_name);
    if !initial.exists() && !reserved.contains(&initial) {
        return initial;
    }
    let file = Path::new(file_name);
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("download");
    let extension = file.extension().and_then(|extension| extension.to_str());
    for index in 1..10_000 {
        let candidate = match extension {
            Some(extension) if !extension.is_empty() => {
                directory.join(format!("{stem} ({index}).{extension}"))
            }
            _ => directory.join(format!("{stem} ({index})")),
        };
        if !candidate.exists() && !reserved.contains(&candidate) {
            return candidate;
        }
    }
    directory.join(format!("{stem}-{}", uuid::Uuid::new_v4().simple()))
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

fn hide_all_workspace_webviews<R: Runtime>(
    app: &AppHandle<R>,
    state: &WorkspaceWebviewState,
) -> Result<(), String> {
    let labels: Vec<String> = registry(state)?
        .values()
        .map(|entry| entry.label.clone())
        .collect();
    for label in labels {
        if let Some(webview) = app.get_webview(&label) {
            // A child can already be closing while React switches tabs. Do
            // not abort the rest of the cleanup in that race.
            let _ = webview.hide();
        }
    }
    Ok(())
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
    let download_directory = workspace_download_dir(&app)?;
    let download_tracker = Arc::new(Mutex::new(DownloadTracker::default()));
    // Start affected pages on about:blank so the WebView2 request hook is
    // installed before the first HTTPS navigation. Otherwise the initial
    // document can race ahead and receive ESA's 520 response.
    let delayed_navigation = needs_ldxp_request_header(&url);
    let initial_url = if delayed_navigation {
        "about:blank"
            .parse::<tauri::Url>()
            .expect("about:blank is a valid WebView URL")
    } else {
        url.clone()
    };
    let mut builder = WebviewBuilder::new(webview_label.clone(), WebviewUrl::External(initial_url))
        .focused(false)
        .devtools(cfg!(debug_assertions))
        .initialization_script_for_all_frames(copy_initialization_script(&copy_proof))
        .on_navigation(|url| url.as_str() == "about:blank" || is_allowed_external_url(url))
        .on_new_window(move |url, _| {
            // Do not let WebView2 create an unmanaged native popup. In
            // particular, allowing about:blank creates a full-window popup
            // that steals focus from every workspace tab. Real destinations
            // are emitted and opened as managed workspace tabs below.
            if url.as_str() == "about:blank" {
                return NewWindowResponse::Deny;
            }
            emit_new_tab_requested(&new_tab_app, &new_tab_source_tab_id, &url);
            NewWindowResponse::Deny
        })
        .on_download(move |webview, event| {
            let origin = webview_origin(&webview);
            match event {
                DownloadEvent::Requested { url, destination } => {
                    let file_name = safe_file_name(Some(destination.as_path()), &url)
                        .unwrap_or_else(|| {
                            format!("aether-download-{}.bin", Utc::now().timestamp_millis())
                        });
                    let path = download_tracker
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .reserve(&url, &download_directory, &file_name);
                    *destination = path.clone();
                    emit_activity(
                        &webview,
                        activity(
                            download_tab_id.clone(),
                            "download",
                            "requested",
                            origin,
                            safe_file_name(Some(path.as_path()), &url),
                            None,
                        ),
                    );
                }
                DownloadEvent::Finished { url, path, success } => {
                    let completed_path = download_tracker
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .finish(&url, path.as_deref());
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
    if request.use_outbound_proxy {
        let settings = app.state::<crate::AppState>().outbound_proxy.load_full();
        if let Some(proxy_url) = crate::outbound_proxy::webview_proxy_url(&settings)? {
            builder = builder.proxy_url(proxy_url);
        }
    }

    let webview = caller
        .window()
        .add_child(
            builder,
            LogicalPosition::new(bounds.x, bounds.y),
            LogicalSize::new(bounds.width, bounds.height),
        )
        .map_err(|error| error.to_string())?;
    if let Err(error) = install_ldxp_request_header(&webview) {
        let _ = webview.close();
        return Err(error);
    }
    if delayed_navigation {
        if let Err(error) = webview.navigate(url) {
            let message = error.to_string();
            let _ = webview.close();
            return Err(message);
        }
    }

    registry(&state)?.insert(
        request.tab_id.clone(),
        ManagedWebview {
            label: webview_label.clone(),
            copy_proof,
        },
    );
    if request.visible {
        if let Err(error) = show_exclusively(&app, &state, &webview) {
            registry(&state)?.remove(&request.tab_id);
            let _ = webview.close();
            return Err(error);
        }
    } else {
        if let Err(error) = webview.hide() {
            registry(&state)?.remove(&request.tab_id);
            let _ = webview.close();
            return Err(error.to_string());
        }
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
            let _ = webview.close();
        }
        registry(&state)?.remove(&tab_id);
    }

    let Some(active) = active else {
        hide_all_workspace_webviews(&app, &state)?;
        caller.set_focus().map_err(|error| error.to_string())?;
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
        if let Err(error) = webview.set_bounds(bounds.as_rect()) {
            let message = error.to_string();
            let _ = hide_all_workspace_webviews(&app, &state);
            let _ = caller.set_focus();
            return Err(message);
        }
        if let Err(error) = show_exclusively(&app, &state, &webview) {
            let _ = hide_all_workspace_webviews(&app, &state);
            let _ = caller.set_focus();
            return Err(error);
        }
        return Ok(());
    }

    // Hide existing native children before creating a new one. If creation
    // fails (for example, when a configured proxy is rejected by WebView2),
    // no stale page can remain above the main UI and capture all input.
    hide_all_workspace_webviews(&app, &state)?;
    if let Err(error) = create_workspace_webview(
        caller.clone(),
        app.clone(),
        state,
        CreateWorkspaceWebviewRequest {
            tab_id: active.tab_id,
            url: active.url,
            bounds,
            visible: true,
            use_outbound_proxy: active.use_outbound_proxy,
        },
    )
    .await
    {
        let _ = caller.set_focus();
        return Err(error);
    }
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

#[tauri::command]
pub(crate) fn reload_workspace_webview<R: Runtime>(
    caller: Webview<R>,
    app: AppHandle<R>,
    state: State<'_, WorkspaceWebviewState>,
    tab_id: String,
) -> Result<bool, String> {
    ensure_main_caller(&caller)?;
    let Some(webview) = managed_webview(&app, &state, &tab_id)? else {
        return Ok(false);
    };
    webview.reload().map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ldxp_header_workaround_is_domain_scoped() {
        assert!(needs_ldxp_request_header(
            &"https://pay.ldxp.cn/".parse().unwrap()
        ));
        assert!(needs_ldxp_request_header(
            &"https://www.ldxp.cn/shopApi/Shop/info".parse().unwrap()
        ));
        assert!(!needs_ldxp_request_header(
            &"https://example.com/?next=https://pay.ldxp.cn"
                .parse()
                .unwrap()
        ));
        assert!(!needs_ldxp_request_header(
            &"https://pay.ldxp.cn.evil.example/".parse().unwrap()
        ));
    }

    #[test]
    fn sanitizes_windows_download_names() {
        assert_eq!(
            sanitize_download_file_name(" report?.json "),
            Some("report.json".to_string())
        );
        assert_eq!(
            sanitize_download_file_name("CON.txt"),
            Some("_CON.txt".to_string())
        );
        assert_eq!(sanitize_download_file_name(".."), None);
    }

    #[test]
    fn reserves_distinct_paths_for_concurrent_downloads() {
        let directory = Path::new("__aether_download_test__");
        let mut reserved = HashSet::new();
        let first = unique_download_path(directory, "accounts.json", &reserved);
        reserved.insert(first.clone());
        let second = unique_download_path(directory, "accounts.json", &reserved);

        assert_eq!(first, directory.join("accounts.json"));
        assert_eq!(second, directory.join("accounts (1).json"));
    }

    #[test]
    fn tracker_recovers_requested_path_when_platform_omits_completed_path() {
        let url = "https://example.com/accounts.json".parse().unwrap();
        let directory = Path::new("__aether_download_test__");
        let mut tracker = DownloadTracker::default();
        let requested = tracker.reserve(&url, directory, "accounts.json");

        assert_eq!(tracker.finish(&url, None), Some(requested));
        assert!(tracker.by_url.is_empty());
        assert!(tracker.reserved.is_empty());
    }
}
