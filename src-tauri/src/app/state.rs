use super::clipboard::ClipboardImportState;
use crate::capacity::CapacityRegistry;
use crate::db::Db;
use arc_swap::ArcSwap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub db: Arc<Db>,
    pub app_data_dir: PathBuf,
    pub client: reqwest::Client,
    pub proxy_port: u16,
    pub proxy_profile: &'static str,
    pub capacity: Arc<CapacityRegistry>,
    pub access_token: Arc<ArcSwap<String>>,
    pub proxy_running: Arc<AtomicBool>,
    pub(super) clipboard_import: Mutex<ClipboardImportState>,
    pub(super) clipboard_reading: AtomicBool,
}
