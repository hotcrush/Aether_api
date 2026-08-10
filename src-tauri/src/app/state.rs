use super::clipboard::ClipboardImportState;
use crate::capacity::CapacityRegistry;
use crate::cost_guard::CostGuardSettings;
use crate::db::Db;
use crate::image_generation::ImageGenerationSettings;
use crate::oauth::OpenAIOAuthSessions;
use crate::outbound_proxy::OutboundProxySettings;
use arc_swap::ArcSwap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub db: Arc<Db>,
    pub app_data_dir: PathBuf,
    pub client: Arc<ArcSwap<reqwest::Client>>,
    pub proxy_client: Arc<ArcSwap<reqwest::Client>>,
    pub outbound_proxy: Arc<ArcSwap<OutboundProxySettings>>,
    pub image_generation: Arc<ArcSwap<ImageGenerationSettings>>,
    pub codex_version: Arc<ArcSwap<String>>,
    pub proxy_port: u16,
    pub proxy_profile: &'static str,
    pub capacity: Arc<CapacityRegistry>,
    pub cost_guard: Arc<ArcSwap<CostGuardSettings>>,
    pub oauth_sessions: OpenAIOAuthSessions,
    pub openai_callback_ready: Arc<AtomicBool>,
    pub access_token: Arc<ArcSwap<String>>,
    pub proxy_running: Arc<AtomicBool>,
    pub(super) clipboard_import: Mutex<ClipboardImportState>,
    pub(super) clipboard_reading: AtomicBool,
}
