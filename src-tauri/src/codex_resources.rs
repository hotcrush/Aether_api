use crate::db::Db;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

const PROMPT_STORE_KEY: &str = "aether:codex_prompt_presets:v1";
const MAX_PROMPT_BYTES: usize = 2 * 1024 * 1024;
const MAX_SKILL_MANIFEST_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CodexPromptPreset {
    pub id: String,
    pub name: String,
    pub content: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct CodexPromptStore {
    prompts: Vec<CodexPromptPreset>,
    active_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexPromptState {
    pub prompts: Vec<CodexPromptPreset>,
    pub active_id: Option<String>,
    pub file_path: String,
    pub file_exists: bool,
    pub current_content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexSkillEntry {
    pub directory: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexSkillState {
    pub skills: Vec<CodexSkillEntry>,
    pub skills_dir: String,
    pub disabled_dir: String,
}

pub fn prompt_state(db: &Db) -> Result<CodexPromptState, String> {
    let mut store = load_prompt_store(db)?;
    let prompt_path = codex_dir()?.join("AGENTS.md");
    let current_content = read_optional_text(&prompt_path)?.unwrap_or_default();

    // Codex or the user may edit AGENTS.md outside Aether. Treat the live file
    // as authoritative for the active preset so reopening settings never
    // silently overwrites a newer external edit.
    let mut changed = false;
    if let Some(active_id) = store.active_id.as_deref() {
        if let Some(active) = store.prompts.iter_mut().find(|item| item.id == active_id) {
            if prompt_path.exists() && active.content != current_content {
                active.content = current_content.clone();
                active.updated_at = chrono::Utc::now().to_rfc3339();
                changed = true;
            }
        } else {
            store.active_id = None;
            changed = true;
        }
    }
    if changed {
        save_prompt_store(db, &store)?;
    }
    Ok(build_prompt_state(store, prompt_path, current_content))
}

pub fn save_prompt(
    db: &Db,
    id: Option<String>,
    name: String,
    content: String,
    activate: bool,
) -> Result<CodexPromptState, String> {
    let name = validate_prompt_name(&name)?;
    validate_prompt_content(&content)?;
    let mut store = load_prompt_store(db)?;
    let id = id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(existing) = store.prompts.iter_mut().find(|item| item.id == id) {
        existing.name = name;
        existing.content = content.clone();
        existing.updated_at = now;
    } else {
        store.prompts.push(CodexPromptPreset {
            id: id.clone(),
            name,
            content: content.clone(),
            updated_at: now,
        });
    }

    let prompt_path = codex_dir()?.join("AGENTS.md");
    if activate || store.active_id.as_deref() == Some(id.as_str()) {
        write_text(&prompt_path, &content)?;
        store.active_id = Some(id);
    }
    save_prompt_store(db, &store)?;
    let current_content = read_optional_text(&prompt_path)?.unwrap_or_default();
    Ok(build_prompt_state(store, prompt_path, current_content))
}

pub fn activate_prompt(db: &Db, id: &str) -> Result<CodexPromptState, String> {
    let mut store = load_prompt_store(db)?;
    let id = id.trim();
    let preset = store
        .prompts
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| "提示词预设不存在".to_string())?;
    let prompt_path = codex_dir()?.join("AGENTS.md");
    write_text(&prompt_path, &preset.content)?;
    store.active_id = Some(preset.id);
    save_prompt_store(db, &store)?;
    Ok(build_prompt_state(store, prompt_path, preset.content))
}

pub fn import_current_prompt(db: &Db) -> Result<CodexPromptState, String> {
    let prompt_path = codex_dir()?.join("AGENTS.md");
    let content = read_optional_text(&prompt_path)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "当前 AGENTS.md 为空，无法导入".to_string())?;
    validate_prompt_content(&content)?;
    let mut store = load_prompt_store(db)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    store.prompts.push(CodexPromptPreset {
        id: id.clone(),
        name: unique_import_name(&store.prompts),
        content: content.clone(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    });
    store.active_id = Some(id);
    save_prompt_store(db, &store)?;
    Ok(build_prompt_state(store, prompt_path, content))
}

pub fn delete_prompt(db: &Db, id: &str) -> Result<CodexPromptState, String> {
    let mut store = load_prompt_store(db)?;
    let previous_len = store.prompts.len();
    store.prompts.retain(|item| item.id != id.trim());
    if store.prompts.len() == previous_len {
        return Err("提示词预设不存在".to_string());
    }
    if store.active_id.as_deref() == Some(id.trim()) {
        store.active_id = None;
    }
    save_prompt_store(db, &store)?;
    let prompt_path = codex_dir()?.join("AGENTS.md");
    let current_content = read_optional_text(&prompt_path)?.unwrap_or_default();
    Ok(build_prompt_state(store, prompt_path, current_content))
}

pub fn skill_state() -> Result<CodexSkillState, String> {
    let codex_dir = codex_dir()?;
    let skills_dir = codex_dir.join("skills");
    let disabled_dir = codex_dir.join(".aether-disabled-skills");
    std::fs::create_dir_all(&skills_dir)
        .map_err(|error| format!("创建 {} 失败: {error}", skills_dir.display()))?;
    std::fs::create_dir_all(&disabled_dir)
        .map_err(|error| format!("创建 {} 失败: {error}", disabled_dir.display()))?;

    let mut skills = scan_skills(&skills_dir, true)?;
    skills.extend(scan_skills(&disabled_dir, false)?);
    skills.sort_by(|left, right| {
        right
            .enabled
            .cmp(&left.enabled)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(CodexSkillState {
        skills,
        skills_dir: skills_dir.display().to_string(),
        disabled_dir: disabled_dir.display().to_string(),
    })
}

pub fn set_skill_enabled(directory: &str, enabled: bool) -> Result<CodexSkillState, String> {
    let directory = validate_skill_directory(directory)?;
    let codex_dir = codex_dir()?;
    let skills_dir = codex_dir.join("skills");
    let disabled_dir = codex_dir.join(".aether-disabled-skills");
    std::fs::create_dir_all(&skills_dir)
        .map_err(|error| format!("创建 {} 失败: {error}", skills_dir.display()))?;
    std::fs::create_dir_all(&disabled_dir)
        .map_err(|error| format!("创建 {} 失败: {error}", disabled_dir.display()))?;
    let (source, destination) = if enabled {
        (disabled_dir.join(&directory), skills_dir.join(&directory))
    } else {
        (skills_dir.join(&directory), disabled_dir.join(&directory))
    };
    if !source.join("SKILL.md").is_file() {
        return Err(format!("Skill 不存在或缺少 SKILL.md: {directory}"));
    }
    if destination.exists() {
        return Err(format!("目标目录已存在，无法切换 Skill: {directory}"));
    }
    std::fs::rename(&source, &destination).map_err(|error| {
        format!(
            "移动 Skill {} 到 {} 失败: {error}",
            source.display(),
            destination.display()
        )
    })?;
    skill_state()
}

fn build_prompt_state(
    mut store: CodexPromptStore,
    prompt_path: PathBuf,
    current_content: String,
) -> CodexPromptState {
    store.prompts.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.name.cmp(&right.name))
    });
    CodexPromptState {
        prompts: store.prompts,
        active_id: store.active_id,
        file_exists: prompt_path.exists(),
        file_path: prompt_path.display().to_string(),
        current_content,
    }
}

fn load_prompt_store(db: &Db) -> Result<CodexPromptStore, String> {
    let Some(raw) = db
        .get_setting(PROMPT_STORE_KEY)
        .map_err(|error| format!("读取提示词预设失败: {error}"))?
    else {
        return Ok(CodexPromptStore::default());
    };
    serde_json::from_str(&raw).map_err(|error| format!("解析提示词预设失败: {error}"))
}

fn save_prompt_store(db: &Db, store: &CodexPromptStore) -> Result<(), String> {
    let encoded =
        serde_json::to_string(store).map_err(|error| format!("序列化提示词预设失败: {error}"))?;
    db.set_setting(PROMPT_STORE_KEY, &encoded)
        .map_err(|error| format!("保存提示词预设失败: {error}"))
}

fn unique_import_name(prompts: &[CodexPromptPreset]) -> String {
    let base = "当前 AGENTS.md";
    if prompts.iter().all(|item| item.name != base) {
        return base.to_string();
    }
    for index in 2..=999 {
        let candidate = format!("{base} {index}");
        if prompts.iter().all(|item| item.name != candidate) {
            return candidate;
        }
    }
    format!("{base} {}", chrono::Utc::now().timestamp())
}

fn validate_prompt_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("请输入提示词名称".to_string());
    }
    if name.chars().count() > 80 {
        return Err("提示词名称不能超过 80 个字符".to_string());
    }
    Ok(name.to_string())
}

fn validate_prompt_content(value: &str) -> Result<(), String> {
    if value.len() > MAX_PROMPT_BYTES {
        return Err("提示词内容不能超过 2 MiB".to_string());
    }
    Ok(())
}

fn scan_skills(root: &Path, enabled: bool) -> Result<Vec<CodexSkillEntry>, String> {
    let mut entries = Vec::new();
    let children = std::fs::read_dir(root)
        .map_err(|error| format!("读取 {} 失败: {error}", root.display()))?;
    for child in children {
        let child = child.map_err(|error| format!("读取 Skill 目录失败: {error}"))?;
        let directory = child.file_name().to_string_lossy().to_string();
        if directory.starts_with('.') || !child.path().is_dir() {
            continue;
        }
        let manifest = child.path().join("SKILL.md");
        if !manifest.is_file() {
            continue;
        }
        let (name, description) = read_skill_metadata(&manifest, &directory);
        entries.push(CodexSkillEntry {
            directory,
            name,
            description,
            enabled,
            path: child.path().display().to_string(),
        });
    }
    Ok(entries)
}

fn read_skill_metadata(path: &Path, fallback_name: &str) -> (String, String) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (fallback_name.to_string(), String::new());
    };
    if metadata.len() > MAX_SKILL_MANIFEST_BYTES {
        return (fallback_name.to_string(), String::new());
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return (fallback_name.to_string(), String::new());
    };
    let mut name = None;
    let mut description = None;
    let mut lines = content.lines();
    if lines.next().map(str::trim) == Some("---") {
        for line in lines.by_ref() {
            let line = line.trim();
            if line == "---" {
                break;
            }
            if name.is_none() {
                name = frontmatter_scalar(line, "name");
            }
            if description.is_none() {
                description = frontmatter_scalar(line, "description");
            }
        }
    }
    let fallback_description = lines
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default()
        .to_string();
    (
        name.filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback_name.to_string()),
        description
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback_description),
    )
}

fn frontmatter_scalar(line: &str, key: &str) -> Option<String> {
    let value = line.strip_prefix(key)?.strip_prefix(':')?.trim();
    if value.is_empty() || matches!(value, "|" | ">" | "|-" | ">-") {
        return None;
    }
    Some(
        value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value)
            .trim()
            .to_string(),
    )
}

fn validate_skill_directory(value: &str) -> Result<String, String> {
    let value = value.trim();
    let mut components = Path::new(value).components();
    let valid = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !value.starts_with('.');
    if !valid {
        return Err("Skill 目录名无效".to_string());
    }
    Ok(value.to_string())
}

fn codex_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| "无法定位 Codex 配置目录".to_string())?;
    Ok(home.join(".codex"))
}

fn read_optional_text(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))
}

fn write_text(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败: {error}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|error| format!("写入 {} 失败: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{frontmatter_scalar, validate_skill_directory};

    #[test]
    fn parses_skill_frontmatter_scalars() {
        assert_eq!(
            frontmatter_scalar("name: review", "name").as_deref(),
            Some("review")
        );
        assert_eq!(
            frontmatter_scalar("description: \"Review code\"", "description").as_deref(),
            Some("Review code")
        );
    }

    #[test]
    fn rejects_skill_path_traversal() {
        assert!(validate_skill_directory("review").is_ok());
        assert!(validate_skill_directory("../review").is_err());
        assert!(validate_skill_directory(".system").is_err());
    }
}
