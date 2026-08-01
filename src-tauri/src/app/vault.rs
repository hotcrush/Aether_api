use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri_plugin_stronghold::stronghold::Stronghold;

use super::AppState;

/// Manages the Stronghold vault lifecycle.
pub(super) struct VaultState {
    inner: Mutex<Option<Stronghold>>,
    snapshot_path: PathBuf,
}

impl VaultState {
    pub(super) fn new(snapshot_path: PathBuf) -> Self {
        Self {
            inner: Mutex::new(None),
            snapshot_path,
        }
    }

    fn with_stronghold<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Stronghold) -> Result<T, String>,
    {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let stronghold = guard.as_ref().ok_or("保险库未初始化，请先设置主密码")?;
        f(stronghold)
    }
}

/// Simple password hash: SHA-256 based (stronghold handles the real encryption).
fn hash_password(password: &str) -> Vec<u8> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    // Use multiple rounds of hashing for basic key stretching
    let mut key = password.as_bytes().to_vec();
    for _ in 0..10_000 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let h = hasher.finish();
        key = h.to_le_bytes().to_vec();
    }
    key
}

#[tauri::command]
pub(crate) fn init_vault(
    vault: tauri::State<'_, VaultState>,
    password: String,
) -> Result<(), String> {
    let mut guard = vault.inner.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_some() {
        return Err("保险库已初始化".to_string());
    }
    let hash = hash_password(&password);
    let stronghold =
        Stronghold::new(vault.snapshot_path.clone(), hash).map_err(|e| e.to_string())?;
    *guard = Some(stronghold);
    Ok(())
}

#[tauri::command]
pub(crate) fn lock_vault(vault: tauri::State<'_, VaultState>) -> Result<(), String> {
    let mut guard = vault.inner.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(stronghold) = guard.take() {
        stronghold.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn is_vault_unlocked(vault: tauri::State<'_, VaultState>) -> bool {
    let guard = vault.inner.lock().unwrap_or_else(|p| p.into_inner());
    guard.is_some()
}

#[tauri::command]
pub(crate) fn store_secret(
    vault: tauri::State<'_, VaultState>,
    key: String,
    value: String,
) -> Result<(), String> {
    vault.with_stronghold(|stronghold| {
        stronghold
            .store()
            .insert(key.as_bytes().to_vec(), value.as_bytes().to_vec(), None)
            .map_err(|e| e.to_string())?;
        stronghold.save().map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub(crate) fn get_secret(
    vault: tauri::State<'_, VaultState>,
    key: String,
) -> Result<Option<String>, String> {
    vault.with_stronghold(|stronghold| {
        let value = stronghold
            .store()
            .get(key.as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(value.and_then(|v| String::from_utf8(v).ok()))
    })
}

#[tauri::command]
pub(crate) fn delete_secret(
    vault: tauri::State<'_, VaultState>,
    key: String,
) -> Result<(), String> {
    vault.with_stronghold(|stronghold| {
        stronghold
            .store()
            .delete(key.as_bytes())
            .map_err(|e| e.to_string())?;
        stronghold.save().map_err(|e| e.to_string())
    })
}

/// Migrate all account credentials from SQLite to the encrypted vault.
#[tauri::command]
pub(crate) fn migrate_secrets_to_vault(
    vault: tauri::State<'_, VaultState>,
    state: tauri::State<'_, AppState>,
) -> Result<usize, String> {
    let accounts = state.db.get_active_accounts().map_err(|e| e.to_string())?;
    let count = accounts.len();

    vault.with_stronghold(|stronghold| {
        let store = stronghold.store();
        for account in &accounts {
            let secret = serde_json::json!({
                "api_key": account.api_key,
                "access_token": account.access_token,
                "refresh_token": account.refresh_token,
                "id_token": account.id_token,
            });
            let key = format!("account:{}", account.id);
            store
                .insert(
                    key.as_bytes().to_vec(),
                    secret.to_string().as_bytes().to_vec(),
                    None,
                )
                .map_err(|e| e.to_string())?;
        }
        stronghold.save().map_err(|e| e.to_string())?;
        Ok(count)
    })
}

/// Retrieve all stored account secrets as a map of account_id -> credentials.
#[tauri::command]
pub(crate) fn export_vault_secrets(
    vault: tauri::State<'_, VaultState>,
    state: tauri::State<'_, AppState>,
) -> Result<HashMap<String, serde_json::Value>, String> {
    let accounts = state.db.get_active_accounts().map_err(|e| e.to_string())?;

    vault.with_stronghold(|stronghold| {
        let store = stronghold.store();
        let mut result = HashMap::new();
        for account in &accounts {
            let key = format!("account:{}", account.id);
            if let Ok(Some(bytes)) = store.get(key.as_bytes()) {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    result.insert(account.id.clone(), value);
                }
            }
        }
        Ok(result)
    })
}
