use rusqlite::{backup::Backup, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use toml_edit::DocumentMut;

pub const UNIFIED_CODEX_PROVIDER_ID: &str = "custom";

const OFFICIAL_CODEX_PROVIDER_ID: &str = "openai";
const LEGACY_AETHER_PROVIDER_ID: &str = "aether";
const HISTORY_MIGRATION_NAME: &str = "codex-official-history-unify-v1";
const HISTORY_RESTORE_BACKUP_NAME: &str = "codex-official-history-unify-restore-v1";
const CODEX_STATE_DB_FILENAME: &str = "state_5.sqlite";
const CODEX_SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";
const STATE_DB_ID_CHUNK: usize = 500;

static CODEX_HISTORY_OP_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize)]
pub struct CodexSessionHistoryStatus {
    pub active: bool,
    pub backup_available: bool,
    pub provider_id: String,
    pub codex_dir: String,
    pub sessions_path: String,
    pub archived_sessions_path: String,
    pub state_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CodexSessionHistoryMigrationResult {
    pub migrated_jsonl_files: usize,
    pub migrated_state_rows: usize,
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CodexSessionHistoryRestoreResult {
    pub restored_jsonl_files: usize,
    pub restored_state_rows: usize,
    pub skipped_reason: Option<String>,
}

pub fn session_history_status(app_data_dir: &Path) -> Result<CodexSessionHistoryStatus, String> {
    let codex_dir = codex_dir()?;
    let config_text = read_optional(&codex_dir.join("config.toml"))?.unwrap_or_default();
    let state_paths = codex_state_db_paths(&codex_dir, &config_text)
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    Ok(CodexSessionHistoryStatus {
        active: config_routes_unified(&config_text),
        backup_available: has_unify_history_backup(app_data_dir)?,
        provider_id: UNIFIED_CODEX_PROVIDER_ID.to_string(),
        sessions_path: codex_dir.join("sessions").display().to_string(),
        archived_sessions_path: codex_dir.join("archived_sessions").display().to_string(),
        codex_dir: codex_dir.display().to_string(),
        state_paths,
    })
}

pub fn has_unify_history_backup(app_data_dir: &Path) -> Result<bool, String> {
    let codex_dir_key = canonical_dir_string(&codex_dir()?);
    Ok(has_unify_history_backup_for_dir(
        &history_backup_parent(app_data_dir),
        &codex_dir_key,
    ))
}

pub fn migrate_existing_history(
    app_data_dir: &Path,
) -> Result<CodexSessionHistoryMigrationResult, String> {
    let _guard = lock_history_op();
    let codex_dir = codex_dir()?;
    let config_text = read_optional(&codex_dir.join("config.toml"))?.unwrap_or_default();
    if !config_routes_unified(&config_text) {
        return Ok(CodexSessionHistoryMigrationResult {
            skipped_reason: Some("not_unified".to_string()),
            ..Default::default()
        });
    }

    let source_provider_ids = BTreeSet::from([
        OFFICIAL_CODEX_PROVIDER_ID.to_string(),
        LEGACY_AETHER_PROVIDER_ID.to_string(),
    ]);
    let backup_root = migration_backup_root(app_data_dir, HISTORY_MIGRATION_NAME);
    let migrated_jsonl_files =
        migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)?;
    let migrated_state_rows = migrate_codex_state_dbs(
        &codex_dir,
        &config_text,
        &source_provider_ids,
        &backup_root,
    )?;
    write_backup_generation_meta(&backup_root, &canonical_dir_string(&codex_dir))?;

    if migrated_jsonl_files == 0 && migrated_state_rows == 0 {
        return Ok(CodexSessionHistoryMigrationResult {
            skipped_reason: Some("nothing_to_migrate".to_string()),
            ..Default::default()
        });
    }

    Ok(CodexSessionHistoryMigrationResult {
        migrated_jsonl_files,
        migrated_state_rows,
        skipped_reason: None,
    })
}

pub fn restore_official_history(
    app_data_dir: &Path,
) -> Result<CodexSessionHistoryRestoreResult, String> {
    let _guard = lock_history_op();
    let codex_dir = codex_dir()?;
    let config_text = read_optional(&codex_dir.join("config.toml"))?.unwrap_or_default();
    let codex_dir_key = canonical_dir_string(&codex_dir);
    let (official_session_ids, official_thread_ids) =
        collect_official_ledger(&history_backup_parent(app_data_dir), &codex_dir_key)?;
    if official_session_ids.is_empty() && official_thread_ids.is_empty() {
        return Ok(CodexSessionHistoryRestoreResult {
            skipped_reason: Some("no_backup_ledger".to_string()),
            ..Default::default()
        });
    }

    let restore_backup_root = migration_backup_root(app_data_dir, HISTORY_RESTORE_BACKUP_NAME);
    let mut files = Vec::new();
    collect_jsonl_files(&codex_dir.join("sessions"), &mut files, 0, 8);
    collect_jsonl_files(&codex_dir.join("archived_sessions"), &mut files, 0, 4);

    let mut restored_jsonl_files = 0;
    for file_path in files {
        if rewrite_codex_session_file_lines(&file_path, &codex_dir, &restore_backup_root, |line| {
            rewrite_codex_session_meta_line_for_restore(line, &official_session_ids)
        })? {
            restored_jsonl_files += 1;
        }
    }

    let mut restored_state_rows = 0;
    for db_path in codex_state_db_paths(&codex_dir, &config_text) {
        restored_state_rows += restore_codex_state_db_official_threads(
            &db_path,
            &codex_dir,
            &official_thread_ids,
            &restore_backup_root,
        )?;
    }

    if restored_jsonl_files == 0 && restored_state_rows == 0 {
        return Ok(CodexSessionHistoryRestoreResult {
            skipped_reason: Some("nothing_to_restore".to_string()),
            ..Default::default()
        });
    }

    Ok(CodexSessionHistoryRestoreResult {
        restored_jsonl_files,
        restored_state_rows,
        skipped_reason: None,
    })
}

fn lock_history_op() -> MutexGuard<'static, ()> {
    CODEX_HISTORY_OP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn codex_dir() -> Result<PathBuf, String> {
    let home = home_dir().ok_or_else(|| "无法定位用户主目录，不能读取 Codex 会话".to_string())?;
    Ok(home.join(".codex"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))
}

fn config_routes_unified(config_text: &str) -> bool {
    config_text
        .parse::<DocumentMut>()
        .ok()
        .and_then(|doc| {
            doc.get("model_provider")
                .and_then(|item| item.as_str())
                .map(|provider_id| provider_id.trim() == UNIFIED_CODEX_PROVIDER_ID)
        })
        .unwrap_or(false)
}

fn codex_state_db_paths(codex_dir: &Path, config_text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_unique_path(&mut paths, codex_dir.join(CODEX_STATE_DB_FILENAME));
    if let Some(sqlite_home) = sqlite_home_from_config(config_text) {
        push_unique_path(&mut paths, sqlite_home.join(CODEX_STATE_DB_FILENAME));
    } else if let Some(sqlite_home) = sqlite_home_from_env() {
        push_unique_path(&mut paths, sqlite_home.join(CODEX_STATE_DB_FILENAME));
    }
    paths
}

fn sqlite_home_from_config(config_text: &str) -> Option<PathBuf> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let raw = doc.get("sqlite_home")?.as_str()?.trim();
    (!raw.is_empty()).then(|| resolve_user_path(raw))
}

fn sqlite_home_from_env() -> Option<PathBuf> {
    let raw = std::env::var(CODEX_SQLITE_HOME_ENV).ok()?;
    let raw = raw.trim();
    (!raw.is_empty()).then(|| resolve_user_path(raw))
}

fn resolve_user_path(raw: &str) -> PathBuf {
    if raw == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(raw));
    }
    PathBuf::from(raw)
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn history_backup_parent(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("backups").join(HISTORY_MIGRATION_NAME)
}

fn migration_backup_root(app_data_dir: &Path, migration_name: &str) -> PathBuf {
    app_data_dir
        .join("backups")
        .join(migration_name)
        .join(chrono::Local::now().format("%Y%m%d_%H%M%S").to_string())
}

fn canonical_dir_string(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn has_unify_history_backup_for_dir(ledger_parent: &Path, codex_dir_key: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(ledger_parent) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let generation = entry.path();
        generation.is_dir() && backup_generation_matches_dir(&generation, codex_dir_key)
    })
}

fn write_backup_generation_meta(backup_root: &Path, codex_dir_key: &str) -> Result<(), String> {
    if !backup_root.exists() {
        return Ok(());
    }
    let payload = serde_json::json!({ "codexConfigDir": codex_dir_key });
    let bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|error| format!("序列化 Codex 会话备份元数据失败: {error}"))?;
    write_text(&backup_root.join("meta.json"), &bytes)
}

fn backup_generation_matches_dir(generation: &Path, codex_dir_key: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(generation.join("meta.json")) else {
        return true;
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("codexConfigDir")
                .and_then(Value::as_str)
                .map(|dir| dir == codex_dir_key)
        })
        .unwrap_or(true)
}

fn collect_official_ledger(
    ledger_parent: &Path,
    codex_dir_key: &str,
) -> Result<(HashSet<String>, BTreeSet<String>), String> {
    let mut session_ids = HashSet::new();
    let mut thread_ids = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(ledger_parent) else {
        return Ok((session_ids, thread_ids));
    };
    for entry in entries.flatten() {
        let generation = entry.path();
        if !generation.is_dir() || !backup_generation_matches_dir(&generation, codex_dir_key) {
            continue;
        }
        let mut backup_files = Vec::new();
        collect_jsonl_files(&generation.join("jsonl"), &mut backup_files, 0, 10);
        for backup_file in backup_files {
            collect_official_session_ids_from_backup(&backup_file, &mut session_ids);
        }
        let mut backup_dbs = Vec::new();
        collect_files_with_extension(&generation.join("state"), "sqlite", &mut backup_dbs, 0, 4);
        for backup_db in backup_dbs {
            collect_official_thread_ids_from_backup(&backup_db, &mut thread_ids);
        }
    }
    Ok((session_ids, thread_ids))
}

fn collect_official_session_ids_from_backup(path: &Path, session_ids: &mut HashSet<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        if !line.contains("\"session_meta\"") || !line.contains("\"model_provider\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("model_provider").and_then(Value::as_str)
            != Some(OFFICIAL_CODEX_PROVIDER_ID)
        {
            continue;
        }
        if let Some(session_id) = payload.get("id").and_then(Value::as_str) {
            session_ids.insert(session_id.to_string());
        }
    }
}

fn collect_official_thread_ids_from_backup(db_path: &Path, thread_ids: &mut BTreeSet<String>) {
    let Ok(conn) =
        Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return;
    };
    if !table_exists(&conn, "threads").unwrap_or(false)
        || !column_exists(&conn, "threads", "model_provider").unwrap_or(false)
    {
        return;
    }
    let Ok(mut stmt) = conn.prepare("SELECT id FROM threads WHERE model_provider = ?1") else {
        return;
    };
    let Ok(rows) = stmt.query_map([OFFICIAL_CODEX_PROVIDER_ID], |row| row.get::<_, String>(0))
    else {
        return;
    };
    for thread_id in rows.flatten() {
        thread_ids.insert(thread_id);
    }
}

fn migrate_codex_jsonl_files(
    codex_dir: &Path,
    source_provider_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, String> {
    let mut files = Vec::new();
    collect_jsonl_files(&codex_dir.join("sessions"), &mut files, 0, 8);
    collect_jsonl_files(&codex_dir.join("archived_sessions"), &mut files, 0, 4);

    let source_provider_ids = source_provider_ids.iter().cloned().collect::<HashSet<_>>();
    let mut migrated = 0;
    for file_path in files {
        if rewrite_codex_session_file_lines(&file_path, codex_dir, backup_root, |line| {
            rewrite_codex_session_meta_line(line, &source_provider_ids)
        })? {
            migrated += 1;
        }
    }
    Ok(migrated)
}

fn collect_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>, depth: u8, max_depth: u8) {
    if depth > max_depth || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files, depth + 1, max_depth);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

fn collect_files_with_extension(
    dir: &Path,
    extension: &str,
    files: &mut Vec<PathBuf>,
    depth: u8,
    max_depth: u8,
) {
    if depth > max_depth || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_extension(&path, extension, files, depth + 1, max_depth);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
            files.push(path);
        }
    }
}

fn rewrite_codex_session_file_lines(
    path: &Path,
    codex_dir: &Path,
    backup_root: &Path,
    rewrite_line: impl Fn(&str) -> Option<String>,
) -> Result<bool, String> {
    let metadata_before =
        std::fs::metadata(path).map_err(|error| format!("读取 {} 元数据失败: {error}", path.display()))?;
    let modified_before = metadata_before.modified().ok();
    let len_before = metadata_before.len();
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;

    let mut rewritten = String::with_capacity(content.len());
    let mut changed = false;
    for segment in content.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map(|line| (line, "\n"))
            .unwrap_or((segment, ""));
        if let Some(next_line) = rewrite_line(line) {
            rewritten.push_str(&next_line);
            changed = true;
        } else {
            rewritten.push_str(line);
        }
        rewritten.push_str(newline);
    }

    if !changed {
        return Ok(false);
    }

    ensure_codex_session_file_unchanged(path, modified_before, len_before)?;
    backup_codex_jsonl_file(path, codex_dir, backup_root)?;
    ensure_codex_session_file_unchanged(path, modified_before, len_before)?;
    write_text(path, rewritten.as_bytes())?;
    Ok(true)
}

fn ensure_codex_session_file_unchanged(
    path: &Path,
    modified_before: Option<SystemTime>,
    len_before: u64,
) -> Result<(), String> {
    let metadata_after =
        std::fs::metadata(path).map_err(|error| format!("读取 {} 元数据失败: {error}", path.display()))?;
    if metadata_after.modified().ok() != modified_before || metadata_after.len() != len_before {
        return Err(format!("Codex 会话文件迁移期间发生变化: {}", path.display()));
    }
    Ok(())
}

fn rewrite_codex_session_meta_line(
    line: &str,
    source_provider_ids: &HashSet<String>,
) -> Option<String> {
    if !line.contains("\"session_meta\"") || !line.contains("\"model_provider\"") {
        return None;
    }
    let mut value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = value.get_mut("payload")?.as_object_mut()?;
    let current_provider = payload.get("model_provider")?.as_str()?;
    if !source_provider_ids.contains(current_provider) {
        return None;
    }
    payload.insert(
        "model_provider".to_string(),
        Value::String(UNIFIED_CODEX_PROVIDER_ID.to_string()),
    );
    serde_json::to_string(&value).ok()
}

fn rewrite_codex_session_meta_line_for_restore(
    line: &str,
    official_session_ids: &HashSet<String>,
) -> Option<String> {
    if !line.contains("\"session_meta\"") || !line.contains("\"model_provider\"") {
        return None;
    }
    let mut value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = value.get_mut("payload")?.as_object_mut()?;
    if payload.get("model_provider")?.as_str()? != UNIFIED_CODEX_PROVIDER_ID {
        return None;
    }
    let session_id = payload.get("id")?.as_str()?;
    if !official_session_ids.contains(session_id) {
        return None;
    }
    payload.insert(
        "model_provider".to_string(),
        Value::String(OFFICIAL_CODEX_PROVIDER_ID.to_string()),
    );
    serde_json::to_string(&value).ok()
}

fn migrate_codex_state_dbs(
    codex_dir: &Path,
    config_text: &str,
    source_provider_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, String> {
    let mut migrated = 0;
    for db_path in codex_state_db_paths(codex_dir, config_text) {
        migrated += migrate_codex_state_db_provider_bucket(
            &db_path,
            codex_dir,
            source_provider_ids,
            backup_root,
        )?;
    }
    Ok(migrated)
}

fn migrate_codex_state_db_provider_bucket(
    db_path: &Path,
    codex_dir: &Path,
    source_provider_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, String> {
    if !db_path.exists() || source_provider_ids.is_empty() {
        return Ok(0);
    }
    let mut conn = Connection::open(db_path)
        .map_err(|error| format!("打开 Codex state DB 失败: {error}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("设置 Codex state DB busy_timeout 失败: {error}"))?;
    if !table_exists(&conn, "threads")? || !column_exists(&conn, "threads", "model_provider")? {
        return Ok(0);
    }

    let placeholders = placeholders(source_provider_ids.len());
    let count_sql =
        format!("SELECT COUNT(*) FROM threads WHERE model_provider IN ({placeholders})");
    let matching_rows: i64 = conn
        .query_row(
            &count_sql,
            params_from_iter(source_provider_ids.iter()),
            |row| row.get(0),
        )
        .map_err(|error| format!("统计 Codex state DB 待迁移行失败: {error}"))?;
    if matching_rows == 0 {
        return Ok(0);
    }

    backup_codex_state_db(db_path, codex_dir, backup_root, &conn)?;
    let update_sql =
        format!("UPDATE threads SET model_provider = ? WHERE model_provider IN ({placeholders})");
    let mut values = Vec::with_capacity(source_provider_ids.len() + 1);
    values.push(UNIFIED_CODEX_PROVIDER_ID.to_string());
    values.extend(source_provider_ids.iter().cloned());
    let tx = conn
        .transaction()
        .map_err(|error| format!("开启 Codex state DB 迁移事务失败: {error}"))?;
    let changed = tx
        .execute(&update_sql, params_from_iter(values.iter()))
        .map_err(|error| format!("迁移 Codex state DB provider 失败: {error}"))?;
    tx.commit()
        .map_err(|error| format!("提交 Codex state DB 迁移事务失败: {error}"))?;
    Ok(changed)
}

fn restore_codex_state_db_official_threads(
    db_path: &Path,
    codex_dir: &Path,
    official_thread_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, String> {
    if !db_path.exists() || official_thread_ids.is_empty() {
        return Ok(0);
    }
    let mut conn = Connection::open(db_path)
        .map_err(|error| format!("打开 Codex state DB 失败: {error}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|error| format!("设置 Codex state DB busy_timeout 失败: {error}"))?;
    if !table_exists(&conn, "threads")? || !column_exists(&conn, "threads", "model_provider")? {
        return Ok(0);
    }

    let ids: Vec<&String> = official_thread_ids.iter().collect();
    let mut matching_rows = 0_i64;
    for chunk in ids.chunks(STATE_DB_ID_CHUNK) {
        let placeholders = placeholders(chunk.len());
        let count_sql = format!(
            "SELECT COUNT(*) FROM threads WHERE model_provider = ? AND id IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(UNIFIED_CODEX_PROVIDER_ID.to_string());
        values.extend(chunk.iter().map(|id| (*id).clone()));
        let count: i64 = conn
            .query_row(&count_sql, params_from_iter(values.iter()), |row| row.get(0))
            .map_err(|error| format!("统计 Codex state DB 待还原行失败: {error}"))?;
        matching_rows += count;
    }
    if matching_rows == 0 {
        return Ok(0);
    }

    backup_codex_state_db(db_path, codex_dir, backup_root, &conn)?;
    let tx = conn
        .transaction()
        .map_err(|error| format!("开启 Codex state DB 还原事务失败: {error}"))?;
    let mut changed = 0;
    for chunk in ids.chunks(STATE_DB_ID_CHUNK) {
        let placeholders = placeholders(chunk.len());
        let update_sql = format!(
            "UPDATE threads SET model_provider = ? WHERE model_provider = ? AND id IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 2);
        values.push(OFFICIAL_CODEX_PROVIDER_ID.to_string());
        values.push(UNIFIED_CODEX_PROVIDER_ID.to_string());
        values.extend(chunk.iter().map(|id| (*id).clone()));
        changed += tx
            .execute(&update_sql, params_from_iter(values.iter()))
            .map_err(|error| format!("还原 Codex state DB provider 失败: {error}"))?;
    }
    tx.commit()
        .map_err(|error| format!("提交 Codex state DB 还原事务失败: {error}"))?;
    Ok(changed)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table],
        |_| Ok(()),
    )
    .optional()
    .map(|value| value.is_some())
    .map_err(|error| format!("读取 Codex state DB 表信息失败: {error}"))
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("读取 Codex state DB 字段信息失败: {error}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("读取 Codex state DB 字段信息失败: {error}"))?;
    for name in rows {
        if name.map_err(|error| format!("读取 Codex state DB 字段信息失败: {error}"))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn placeholders(count: usize) -> String {
    std::iter::repeat("?")
        .take(count)
        .collect::<Vec<_>>()
        .join(", ")
}

fn backup_codex_jsonl_file(path: &Path, codex_dir: &Path, backup_root: &Path) -> Result<(), String> {
    let backup_path = backup_root
        .join("jsonl")
        .join(relative_backup_path(path, codex_dir));
    copy_existing_file(path, &backup_path)
}

fn backup_codex_state_db(
    db_path: &Path,
    codex_dir: &Path,
    backup_root: &Path,
    source_conn: &Connection,
) -> Result<(), String> {
    let backup_path = backup_root
        .join("state")
        .join(relative_backup_path(db_path, codex_dir));
    if let Some(parent) = backup_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败: {error}", parent.display()))?;
    }
    let mut backup_conn = Connection::open(&backup_path)
        .map_err(|error| format!("创建 Codex state DB 备份失败: {error}"))?;
    let backup = Backup::new(source_conn, &mut backup_conn)
        .map_err(|error| format!("初始化 Codex state DB 备份失败: {error}"))?;
    backup
        .run_to_completion(5, Duration::from_millis(25), None)
        .map_err(|error| format!("写入 Codex state DB 备份失败: {error}"))?;
    Ok(())
}

fn copy_existing_file(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败: {error}", parent.display()))?;
    }
    std::fs::copy(source, target)
        .map(|_| ())
        .map_err(|error| format!("备份 {} 失败: {error}", source.display()))
}

fn write_text(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败: {error}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|error| format!("写入 {} 失败: {error}", path.display()))
}

fn relative_backup_path(path: &Path, codex_dir: &Path) -> PathBuf {
    if let Ok(relative) = path.strip_prefix(codex_dir) {
        return relative.to_path_buf();
    }
    let mut out = PathBuf::from("external");
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(sanitize_component(&prefix.as_os_str().to_string_lossy())),
            Component::RootDir => out.push("_root"),
            Component::Normal(value) => out.push(sanitize_component(&value.to_string_lossy())),
            Component::CurDir | Component::ParentDir => {}
        }
    }
    out
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_official_session_meta_to_unified_bucket() {
        let sources = BTreeSet::from([
            OFFICIAL_CODEX_PROVIDER_ID.to_string(),
            LEGACY_AETHER_PROVIDER_ID.to_string(),
        ])
        .into_iter()
        .collect::<HashSet<_>>();
        let line = r#"{"type":"session_meta","payload":{"id":"thread-1","model_provider":"openai"}}"#;

        let rewritten = rewrite_codex_session_meta_line(line, &sources).unwrap();
        let value: Value = serde_json::from_str(&rewritten).unwrap();

        assert_eq!(
            value.pointer("/payload/model_provider").and_then(Value::as_str),
            Some(UNIFIED_CODEX_PROVIDER_ID)
        );
    }

    #[test]
    fn restore_only_rewrites_ledgered_official_sessions() {
        let mut session_ids = HashSet::new();
        session_ids.insert("thread-1".to_string());
        let official = r#"{"type":"session_meta","payload":{"id":"thread-1","model_provider":"custom"}}"#;
        let third_party = r#"{"type":"session_meta","payload":{"id":"thread-2","model_provider":"custom"}}"#;

        let restored = rewrite_codex_session_meta_line_for_restore(official, &session_ids).unwrap();
        let value: Value = serde_json::from_str(&restored).unwrap();

        assert_eq!(
            value.pointer("/payload/model_provider").and_then(Value::as_str),
            Some(OFFICIAL_CODEX_PROVIDER_ID)
        );
        assert!(rewrite_codex_session_meta_line_for_restore(third_party, &session_ids).is_none());
    }
}
