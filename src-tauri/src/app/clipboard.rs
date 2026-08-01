use super::accounts::import_parsed_accounts;
use super::AppState;
use crate::account_import::{self, ClipboardImportSource, ImportResult, ParsedClipboardImport};
use crate::db::NewAccount;
use serde::Serialize;
use std::collections::{hash_map::DefaultHasher, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{Emitter, EventTarget, Manager, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;

pub(crate) const WEBVIEW_IMPORT_CANDIDATE_EVENT: &str = "webview:import-candidate";
const MAX_AUTO_IMPORT_BYTES: u64 = 10 * 1024 * 1024;
const MAX_PENDING_IMPORTS: usize = 16;
const RECENT_FINGERPRINT_TTL: Duration = Duration::from_secs(5);
const PENDING_IMPORT_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum ImportCandidateTrigger {
    Clipboard,
    Download,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ClipboardAccountSummary {
    name: String,
    email: String,
    account_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ClipboardImportCandidate {
    candidate_id: String,
    source: ClipboardImportSource,
    detected_from: ImportCandidateTrigger,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_name: Option<String>,
    account_count: usize,
    accounts: Vec<ClipboardAccountSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClipboardFingerprint {
    hash: u64,
    length: usize,
}

#[derive(Debug)]
struct PendingClipboardImport {
    candidate_id: String,
    fingerprint: ClipboardFingerprint,
    accounts: Vec<NewAccount>,
    created_at: Instant,
}

#[derive(Debug, Default)]
pub(super) struct ClipboardImportState {
    recent_fingerprints: VecDeque<(ClipboardFingerprint, Instant)>,
    pending: HashMap<String, PendingClipboardImport>,
}

impl ClipboardImportState {
    fn prune_expired(&mut self, now: Instant) {
        while self
            .recent_fingerprints
            .front()
            .is_some_and(|(_, seen_at)| now.duration_since(*seen_at) >= RECENT_FINGERPRINT_TTL)
        {
            self.recent_fingerprints.pop_front();
        }
        self.pending
            .retain(|_, candidate| now.duration_since(candidate.created_at) < PENDING_IMPORT_TTL);
    }

    fn contains_fingerprint(&self, fingerprint: ClipboardFingerprint) -> bool {
        self.pending
            .values()
            .any(|candidate| candidate.fingerprint == fingerprint)
            || self
                .recent_fingerprints
                .iter()
                .any(|(recent, _)| *recent == fingerprint)
    }

    fn can_accept(&mut self, fingerprint: ClipboardFingerprint) -> bool {
        self.prune_expired(Instant::now());
        self.pending.len() < MAX_PENDING_IMPORTS && !self.contains_fingerprint(fingerprint)
    }

    fn mark_recent(&mut self, fingerprint: ClipboardFingerprint, now: Instant) {
        self.recent_fingerprints.push_back((fingerprint, now));
    }

    fn store_candidate(
        &mut self,
        fingerprint: ClipboardFingerprint,
        parsed: ParsedClipboardImport,
        detected_from: ImportCandidateTrigger,
        file_name: Option<String>,
    ) -> Option<ClipboardImportCandidate> {
        let now = Instant::now();
        self.prune_expired(now);
        if self.pending.len() >= MAX_PENDING_IMPORTS || self.contains_fingerprint(fingerprint) {
            return None;
        }

        let candidate_id = uuid::Uuid::new_v4().to_string();
        let candidate = ClipboardImportCandidate {
            candidate_id: candidate_id.clone(),
            source: parsed.source,
            detected_from,
            file_name,
            account_count: parsed.accounts.len(),
            accounts: parsed
                .accounts
                .iter()
                .take(5)
                .map(clipboard_account_summary)
                .collect(),
        };
        self.pending.insert(
            candidate_id.clone(),
            PendingClipboardImport {
                candidate_id,
                fingerprint,
                accounts: parsed.accounts,
                created_at: now,
            },
        );
        Some(candidate)
    }

    fn take_candidate(&mut self, candidate_id: &str) -> Option<Vec<NewAccount>> {
        let now = Instant::now();
        self.prune_expired(now);
        let pending = self.pending.remove(candidate_id)?;
        debug_assert_eq!(pending.candidate_id, candidate_id);
        self.mark_recent(pending.fingerprint, now);
        Some(pending.accounts)
    }

    fn discard_candidate(&mut self, candidate_id: &str) -> bool {
        let now = Instant::now();
        self.prune_expired(now);
        let Some(pending) = self.pending.remove(candidate_id) else {
            return false;
        };
        debug_assert_eq!(pending.candidate_id, candidate_id);
        self.mark_recent(pending.fingerprint, now);
        true
    }
}

struct ClipboardReadGuard<'a>(&'a AtomicBool);

impl Drop for ClipboardReadGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn clipboard_fingerprint(content: &str) -> ClipboardFingerprint {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    ClipboardFingerprint {
        hash: hasher.finish(),
        length: content.len(),
    }
}

fn clipboard_account_summary(account: &NewAccount) -> ClipboardAccountSummary {
    let name = if !account.name.trim().is_empty() {
        account.name.trim().to_string()
    } else if !account.email.trim().is_empty() {
        account.email.trim().to_string()
    } else if account.account_type == "oauth" {
        "OpenAI OAuth".to_string()
    } else {
        "OpenAI 中转站".to_string()
    };
    ClipboardAccountSummary {
        name,
        email: account.email.trim().to_string(),
        account_type: account.account_type.clone(),
    }
}

fn validate_default_priority(default_priority: Option<i64>) -> Result<i64, String> {
    let default_priority = default_priority.unwrap_or(1);
    if !(0..=1000).contains(&default_priority) {
        return Err("默认优先级必须在 0 到 1000 之间".to_string());
    }
    Ok(default_priority)
}

fn detect_import_candidate(
    state: &AppState,
    content: &str,
    detected_from: ImportCandidateTrigger,
    file_name: Option<String>,
) -> Option<ClipboardImportCandidate> {
    let fingerprint = clipboard_fingerprint(content);
    if !state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .can_accept(fingerprint)
    {
        return None;
    }
    let parsed = account_import::parse_clipboard_import(content).ok()?;
    state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .store_candidate(fingerprint, parsed, detected_from, file_name)
}

pub(crate) fn inspect_downloaded_import<R: Runtime>(
    app: tauri::AppHandle<R>,
    path: PathBuf,
    file_name: Option<String>,
) {
    tauri::async_runtime::spawn_blocking(move || {
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_AUTO_IMPORT_BYTES => {
                metadata
            }
            _ => return,
        };
        if metadata.len() == 0 {
            return;
        }
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(_) => return,
        };
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        if file
            .take(MAX_AUTO_IMPORT_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > MAX_AUTO_IMPORT_BYTES
        {
            return;
        }
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => return,
        };
        let state = app.state::<AppState>();
        let Some(candidate) = detect_import_candidate(
            &state,
            &content,
            ImportCandidateTrigger::Download,
            file_name,
        ) else {
            return;
        };
        if let Err(error) = app.emit_to(
            EventTarget::webview("main"),
            WEBVIEW_IMPORT_CANDIDATE_EVENT,
            candidate,
        ) {
            tracing::warn!(%error, "failed to emit WebView import candidate");
        }
    });
}

#[tauri::command]
pub(super) async fn inspect_clipboard_import(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<ClipboardImportCandidate>, String> {
    if state
        .clipboard_reading
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(None);
    }
    let _reading_guard = ClipboardReadGuard(&state.clipboard_reading);
    let clipboard_result =
        tauri::async_runtime::spawn_blocking(move || app.clipboard().read_text())
            .await
            .map_err(|error| format!("读取剪贴板任务失败: {error}"))?;
    let content = match clipboard_result {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };
    Ok(detect_import_candidate(
        &state,
        &content,
        ImportCandidateTrigger::Clipboard,
        None,
    ))
}

#[tauri::command]
pub(super) async fn confirm_clipboard_import(
    state: tauri::State<'_, AppState>,
    candidate_id: String,
    default_priority: Option<i64>,
) -> Result<ImportResult, String> {
    let default_priority = validate_default_priority(default_priority)?;
    let accounts = state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_candidate(&candidate_id)
        .ok_or_else(|| "自动导入候选已失效".to_string())?;
    Ok(import_parsed_accounts(&state, accounts, Vec::new(), default_priority).await)
}

#[tauri::command]
pub(super) fn discard_clipboard_import(
    state: tauri::State<AppState>,
    candidate_id: String,
) -> bool {
    state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .discard_candidate(&candidate_id)
}

#[tauri::command]
pub(super) async fn import_accounts(
    state: tauri::State<'_, AppState>,
    contents: Vec<String>,
    default_priority: Option<i64>,
) -> Result<ImportResult, String> {
    if contents.is_empty() || contents.len() > 20 {
        return Err("请选择 1 到 20 个导入内容".to_string());
    }
    let default_priority = validate_default_priority(default_priority)?;
    let (accounts, parse_errors) = account_import::parse_import_contents(&contents);
    Ok(import_parsed_accounts(&state, accounts, parse_errors, default_priority).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_accounts(count: usize) -> ParsedClipboardImport {
        ParsedClipboardImport {
            source: ClipboardImportSource::Sub2api,
            accounts: (0..count)
                .map(|index| NewAccount {
                    name: format!("Account {index}"),
                    account_type: "oauth".to_string(),
                    access_token: format!("secret-access-{index}"),
                    refresh_token: format!("secret-refresh-{index}"),
                    email: format!("account-{index}@example.com"),
                    ..NewAccount::default()
                })
                .collect(),
        }
    }

    #[test]
    fn clipboard_candidate_is_sanitized_and_limited() {
        let mut state = ClipboardImportState::default();
        let fingerprint = clipboard_fingerprint("candidate-one");
        let candidate = state
            .store_candidate(
                fingerprint,
                parsed_accounts(7),
                ImportCandidateTrigger::Clipboard,
                None,
            )
            .unwrap();
        let serialized = serde_json::to_string(&candidate).unwrap();

        assert_eq!(candidate.account_count, 7);
        assert_eq!(candidate.accounts.len(), 5);
        assert_eq!(
            serde_json::to_value(&candidate).unwrap()["source"],
            "sub2api"
        );
        assert!(!serialized.contains("secret-access"));
        assert!(!serialized.contains("secret-refresh"));
        assert!(state
            .pending
            .get(&candidate.candidate_id)
            .is_some_and(|pending| {
                pending.accounts.len() == 7 && pending.accounts[0].access_token == "secret-access-0"
            }));
    }

    #[test]
    fn clipboard_candidate_dedupes_and_is_consumed_once() {
        let mut state = ClipboardImportState::default();
        let first = clipboard_fingerprint("candidate-one");
        let second = clipboard_fingerprint("candidate-two");

        let candidate = state
            .store_candidate(
                first,
                parsed_accounts(1),
                ImportCandidateTrigger::Clipboard,
                None,
            )
            .unwrap();
        assert!(state
            .store_candidate(
                first,
                parsed_accounts(1),
                ImportCandidateTrigger::Download,
                Some("duplicate.json".to_string()),
            )
            .is_none());
        assert!(state
            .store_candidate(
                second,
                parsed_accounts(1),
                ImportCandidateTrigger::Download,
                Some("second.json".to_string()),
            )
            .is_some());
        assert!(state.take_candidate("wrong-id").is_none());
        assert_eq!(
            state.take_candidate(&candidate.candidate_id).unwrap().len(),
            1
        );
        assert!(state.take_candidate(&candidate.candidate_id).is_none());
    }

    #[test]
    fn discard_only_clears_the_matching_candidate() {
        let mut state = ClipboardImportState::default();
        let fingerprint = clipboard_fingerprint("candidate-one");
        let candidate = state
            .store_candidate(
                fingerprint,
                parsed_accounts(1),
                ImportCandidateTrigger::Clipboard,
                None,
            )
            .unwrap();

        assert!(!state.discard_candidate("wrong-id"));
        assert_eq!(state.pending.len(), 1);
        assert!(state.discard_candidate(&candidate.candidate_id));
        assert!(state.pending.is_empty());
    }

    #[test]
    fn candidates_can_be_confirmed_independently() {
        let mut state = ClipboardImportState::default();
        let first = state
            .store_candidate(
                clipboard_fingerprint("candidate-one"),
                parsed_accounts(1),
                ImportCandidateTrigger::Clipboard,
                None,
            )
            .unwrap();
        let second = state
            .store_candidate(
                clipboard_fingerprint("candidate-two"),
                parsed_accounts(2),
                ImportCandidateTrigger::Download,
                Some("accounts.json".to_string()),
            )
            .unwrap();

        assert_eq!(state.take_candidate(&second.candidate_id).unwrap().len(), 2);
        assert!(state.pending.contains_key(&first.candidate_id));
        assert_eq!(state.take_candidate(&first.candidate_id).unwrap().len(), 1);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn discarding_one_candidate_keeps_the_others() {
        let mut state = ClipboardImportState::default();
        let first = state
            .store_candidate(
                clipboard_fingerprint("candidate-one"),
                parsed_accounts(1),
                ImportCandidateTrigger::Clipboard,
                None,
            )
            .unwrap();
        let second = state
            .store_candidate(
                clipboard_fingerprint("candidate-two"),
                parsed_accounts(1),
                ImportCandidateTrigger::Download,
                Some("accounts.json".to_string()),
            )
            .unwrap();

        assert!(state.discard_candidate(&first.candidate_id));
        assert!(state.pending.contains_key(&second.candidate_id));
        assert_eq!(state.take_candidate(&second.candidate_id).unwrap().len(), 1);
    }
}
