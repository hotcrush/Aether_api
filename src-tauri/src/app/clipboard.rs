use super::accounts::import_parsed_accounts;
use super::AppState;
use crate::account_import::{self, ClipboardImportSource, ImportResult, ParsedClipboardImport};
use crate::db::NewAccount;
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri_plugin_clipboard_manager::ClipboardExt;

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
    accounts: Vec<NewAccount>,
}

#[derive(Debug, Default)]
pub(super) struct ClipboardImportState {
    last_fingerprint: Option<ClipboardFingerprint>,
    pending: Option<PendingClipboardImport>,
}

impl ClipboardImportState {
    fn mark_fingerprint(&mut self, fingerprint: ClipboardFingerprint) -> bool {
        if self.last_fingerprint == Some(fingerprint) {
            return false;
        }
        self.last_fingerprint = Some(fingerprint);
        true
    }

    fn store_candidate(
        &mut self,
        fingerprint: ClipboardFingerprint,
        parsed: ParsedClipboardImport,
    ) -> ClipboardImportCandidate {
        self.last_fingerprint = Some(fingerprint);
        let candidate_id = uuid::Uuid::new_v4().to_string();
        let candidate = ClipboardImportCandidate {
            candidate_id: candidate_id.clone(),
            source: parsed.source,
            account_count: parsed.accounts.len(),
            accounts: parsed
                .accounts
                .iter()
                .take(5)
                .map(clipboard_account_summary)
                .collect(),
        };
        self.pending = Some(PendingClipboardImport {
            candidate_id,
            accounts: parsed.accounts,
        });
        candidate
    }

    fn take_candidate(&mut self, candidate_id: &str) -> Option<Vec<NewAccount>> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.candidate_id == candidate_id)
        {
            return self.pending.take().map(|pending| pending.accounts);
        }
        None
    }

    fn discard_candidate(&mut self, candidate_id: &str) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.candidate_id == candidate_id)
        {
            self.pending = None;
            return true;
        }
        false
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
    let fingerprint = clipboard_fingerprint(&content);

    {
        let mut clipboard_import = state
            .clipboard_import
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !clipboard_import.mark_fingerprint(fingerprint) {
            return Ok(None);
        }
    }

    let parsed = match account_import::parse_clipboard_import(&content) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(None),
    };
    let candidate = state
        .clipboard_import
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .store_candidate(fingerprint, parsed);
    Ok(Some(candidate))
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
        .ok_or_else(|| "剪贴板导入候选已失效".to_string())?;
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
        let candidate = state.store_candidate(fingerprint, parsed_accounts(7));
        let serialized = serde_json::to_string(&candidate).unwrap();

        assert_eq!(candidate.account_count, 7);
        assert_eq!(candidate.accounts.len(), 5);
        assert_eq!(
            serde_json::to_value(&candidate).unwrap()["source"],
            "sub2api"
        );
        assert!(!serialized.contains("secret-access"));
        assert!(!serialized.contains("secret-refresh"));
        assert!(state.pending.as_ref().is_some_and(|pending| {
            pending.accounts.len() == 7 && pending.accounts[0].access_token == "secret-access-0"
        }));
    }

    #[test]
    fn clipboard_candidate_dedupes_and_is_consumed_once() {
        let mut state = ClipboardImportState::default();
        let first = clipboard_fingerprint("candidate-one");
        let second = clipboard_fingerprint("candidate-two");

        assert!(state.mark_fingerprint(first));
        assert!(!state.mark_fingerprint(first));
        assert!(state.mark_fingerprint(second));

        let candidate = state.store_candidate(second, parsed_accounts(1));
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
        let candidate = state.store_candidate(fingerprint, parsed_accounts(1));

        assert!(!state.discard_candidate("wrong-id"));
        assert!(state.pending.is_some());
        assert!(state.discard_candidate(&candidate.candidate_id));
        assert!(state.pending.is_none());
    }
}
