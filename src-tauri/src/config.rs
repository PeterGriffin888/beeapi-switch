//! Dispatch layer: maps frontend tool IDs to concrete writer functions,
//! handles backup of existing configs and surfaces a status view.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::tools::{self, DEFAULT_BASE};

#[derive(Serialize)]
pub struct ApplyResult {
    pub tool: String,
    pub files: Vec<String>,
    pub backup_dir: Option<String>,
}

#[derive(Serialize)]
pub struct ConfigStatus {
    pub tool: String,
    pub configured: bool,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub key_hint: Option<String>,
}

#[derive(Serialize)]
pub struct BackupEntry {
    pub tool: String,
    pub name: String,
    pub path: String,
    pub modified: i64,
    pub files: Vec<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub key_hint: Option<String>,
}

#[derive(Deserialize)]
pub struct RestoreBackupRequest {
    pub tool: String,
    pub path: String,
}

const ALL_TOOLS: &[&str] = &[
    "claude-code",
    "codex",
    "gemini-cli",
    "opencode",
    "openclaw",
    "hermes",
];

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow!("Cannot locate home directory"))
}

fn tool_paths(tool: &str, home: &PathBuf) -> Result<Vec<PathBuf>> {
    Ok(match tool {
        "claude-code" => tools::claude_code_paths(home),
        "codex" => tools::codex_paths(home),
        "gemini-cli" => tools::gemini_paths(home),
        "opencode" => tools::opencode_paths(home),
        "openclaw" => tools::openclaw_paths(home),
        "hermes" => tools::hermes_paths(home),
        other => return Err(anyhow!("Unknown tool: {other}")),
    })
}

pub fn tool_status(tool: &str) -> Result<tools::StatusOutcome> {
    let home = home_dir()?;
    match tool {
        "claude-code" => tools::claude_code_status(&home),
        "codex" => tools::codex_status(&home),
        "gemini-cli" => tools::gemini_status(&home),
        "opencode" => tools::opencode_status(&home),
        "openclaw" => tools::openclaw_status(&home),
        "hermes" => tools::hermes_status(&home),
        other => Err(anyhow!("Unknown tool: {other}")),
    }
}

fn backup_existing(tool: &str, paths: &[PathBuf]) -> Result<Option<PathBuf>> {
    let existing: Vec<&PathBuf> = paths.iter().filter(|p| p.exists()).collect();
    if existing.is_empty() {
        return Ok(None);
    }

    let home = home_dir()?;
    let tool_backup_root = home
        .join(".beeapi-switch")
        .join("backups")
        .join(tool);

    if latest_backup_has_same_files(&tool_backup_root, &existing) {
        return Ok(None);
    }

    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut dir = tool_backup_root.join(&stamp);
    for i in 2..100 {
        if !dir.exists() {
            break;
        }
        dir = tool_backup_root.join(format!("{stamp}-{i}"));
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create backup dir {}", dir.display()))?;

    for src in existing {
        let file_name = src
            .file_name()
            .ok_or_else(|| anyhow!("Cannot get file name for {}", src.display()))?;
        let dst = dir.join(file_name);
        std::fs::copy(src, &dst)
            .with_context(|| format!("Failed to backup {} -> {}", src.display(), dst.display()))?;
    }
    Ok(Some(dir))
}

fn latest_backup_has_same_files(tool_backup_root: &Path, existing: &[&PathBuf]) -> bool {
    if !tool_backup_root.exists() {
        return false;
    }
    let latest = std::fs::read_dir(tool_backup_root)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .max_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });

    let Some(latest) = latest else {
        return false;
    };

    existing.iter().all(|src| {
        let Some(name) = src.file_name().and_then(|s| s.to_str()) else {
            return false;
        };
        let Some(prev) = find_backup_file(&latest, name) else {
            return false;
        };
        let Ok(current) = std::fs::read(src) else {
            return false;
        };
        let Ok(backed_up) = std::fs::read(prev) else {
            return false;
        };
        current == backed_up
    })
}

pub fn apply(tool: &str, api_key: &str, model: &str, base: Option<&str>) -> Result<ApplyResult> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("API Key cannot be empty"));
    }
    if model.trim().is_empty() {
        return Err(anyhow!("Please select a model"));
    }
    let base = base
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BASE);

    let home = home_dir()?;
    let paths = tool_paths(tool, &home)?;
    let backup = backup_existing(tool, &paths)?;

    let outcome = match tool {
        "claude-code" => tools::claude_code_apply(&home, api_key, model, base)?,
        "codex" => tools::codex_apply(&home, api_key, model, base)?,
        "gemini-cli" => tools::gemini_apply(&home, api_key, model, base)?,
        "opencode" => tools::opencode_apply(&home, api_key, model, base)?,
        "openclaw" => tools::openclaw_apply(&home, api_key, model, base)?,
        "hermes" => tools::hermes_apply(&home, api_key, model, base)?,
        other => return Err(anyhow!("Unknown tool: {other}")),
    };

    Ok(ApplyResult {
        tool: tool.to_string(),
        files: outcome
            .files
            .into_iter()
            .map(|p| p.display().to_string())
            .collect(),
        backup_dir: backup.map(|p| p.display().to_string()),
    })
}

pub fn clear(tool: &str) -> Result<()> {
    let home = home_dir()?;
    let paths = tool_paths(tool, &home)?;
    let _ = backup_existing(tool, &paths)?;

    match tool {
        "claude-code" => tools::claude_code_clear(&home)?,
        "codex" => tools::codex_clear(&home)?,
        "gemini-cli" => tools::gemini_clear(&home)?,
        "opencode" => tools::opencode_clear(&home)?,
        "openclaw" => tools::openclaw_clear(&home)?,
        "hermes" => tools::hermes_clear(&home)?,
        other => return Err(anyhow!("Unknown tool: {other}")),
    }
    Ok(())
}

pub fn list_status() -> Result<Vec<ConfigStatus>> {
    let home = home_dir()?;
    let mut out = Vec::with_capacity(ALL_TOOLS.len());
    for tool in ALL_TOOLS {
        let s = match *tool {
            "claude-code" => tools::claude_code_status(&home)?,
            "codex" => tools::codex_status(&home)?,
            "gemini-cli" => tools::gemini_status(&home)?,
            "opencode" => tools::opencode_status(&home)?,
            "openclaw" => tools::openclaw_status(&home)?,
            "hermes" => tools::hermes_status(&home)?,
            _ => continue,
        };
        out.push(ConfigStatus {
            tool: tool.to_string(),
            configured: s.configured,
            base_url: s.base_url,
            model: s.model,
            key_hint: s.key.map(|k| display_key_hint(&k)),
        });
    }
    Ok(out)
}



fn find_backup_file(backup_path: &Path, file_name: &str) -> Option<PathBuf> {
    let direct = backup_path.join(file_name);
    if direct.is_file() {
        return Some(direct);
    }

    fn walk(dir: &Path, file_name: &str) -> Option<PathBuf> {
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|name| name == file_name)
                    .unwrap_or(false)
            {
                return Some(path);
            }
            if path.is_dir() {
                if let Some(found) = walk(&path, file_name) {
                    return Some(found);
                }
            }
        }
        None
    }

    walk(backup_path, file_name)
}

fn read_backup_text(backup_path: &Path, file_name: &str) -> Option<String> {
    find_backup_file(backup_path, file_name).and_then(|path| std::fs::read_to_string(path).ok())
}

fn read_backup_json(backup_path: &Path, file_name: &str) -> Option<serde_json::Value> {
    read_backup_text(backup_path, file_name)
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
}

fn collect_backup_files(backup_path: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let rel = path
                        .strip_prefix(root)
                        .ok()
                        .and_then(|p| p.to_str())
                        .map(|s| s.replace('\\', "/"))
                        .or_else(|| {
                            path.file_name()
                                .and_then(|s| s.to_str())
                                .map(String::from)
                        });
                    if let Some(name) = rel {
                        out.push(name);
                    }
                } else if path.is_dir() {
                    walk(root, &path, out);
                }
            }
        }
    }

    let mut files = Vec::new();
    walk(backup_path, backup_path, &mut files);
    files.sort();
    files
}

fn backup_summary(tool: &str, backup_path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    fn mask_opt(v: Option<String>) -> Option<String> {
        v.filter(|k| !k.trim().is_empty()).map(|k| mask_key(&k))
    }
    fn env_value(text: &str, key: &str) -> Option<String> {
        let needle = format!("{key}=");
        text.lines().find_map(|line| {
            let value = line.trim().strip_prefix(&needle)?.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.trim_matches('"').trim_matches('\'').to_string())
            }
        })
    }
    fn yaml_value(text: &str, key: &str) -> Option<String> {
        let needle = format!("{key}:");
        text.lines().find_map(|line| {
            let trimmed = line.trim();
            let value = trimmed.strip_prefix(&needle)?.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.trim_matches('"').trim_matches('\'').to_string())
            }
        })
    }
    fn json_string(v: &serde_json::Value, path: &[&str]) -> Option<String> {
        let mut current = v;
        for key in path {
            current = current.get(*key)?;
        }
        current.as_str().map(String::from)
    }
    fn env_string(
        env: Option<&serde_json::Map<String, serde_json::Value>>,
        keys: &[&str],
    ) -> Option<String> {
        for key in keys {
            if let Some(value) = env
                .and_then(|e| e.get(*key))
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
            {
                return Some(value.to_string());
            }
        }
        None
    }
    fn codex_provider_base(doc: &toml_edit::DocumentMut, provider: Option<&str>) -> Option<String> {
        let providers = doc
            .get("model_providers")
            .and_then(|mp| mp.as_table())?;
        if let Some(provider) = provider {
            if let Some(base) = providers
                .get(provider)
                .and_then(|p| p.as_table())
                .and_then(|t| t.get("base_url"))
                .and_then(|v| v.as_str())
            {
                return Some(base.to_string());
            }
        }
        providers.iter().find_map(|(_, item)| {
            item.as_table()
                .and_then(|t| t.get("base_url"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
    }
    fn opencode_provider<'a>(
        v: &'a serde_json::Value,
        provider_id: Option<&str>,
    ) -> Option<&'a serde_json::Value> {
        let providers = v.get("provider")?.as_object()?;
        if let Some(provider_id) = provider_id {
            if let Some(provider) = providers.get(provider_id) {
                return Some(provider);
            }
        }
        providers
            .get("beeapi")
            .or_else(|| providers.values().find(|provider| provider.get("options").is_some()))
            .or_else(|| providers.values().next())
    }
    fn chatgpt_login_hint(auth: Option<&serde_json::Value>) -> Option<String> {
        let auth = auth?;
        let mode_is_chatgpt = auth
            .get("auth_mode")
            .and_then(|v| v.as_str())
            .map(|mode| mode.eq_ignore_ascii_case("chatgpt"))
            .unwrap_or(false);
        let has_tokens = auth.get("tokens").is_some()
            || auth.get("id_token").is_some()
            || auth.get("access_token").is_some()
            || auth.get("refresh_token").is_some();
        if !mode_is_chatgpt && !has_tokens {
            return None;
        }
        let email = auth
            .get("email")
            .or_else(|| auth.pointer("/account/email"))
            .or_else(|| auth.pointer("/profile/email"))
            .or_else(|| auth.pointer("/user/email"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        Some(match email {
            Some(email) => format!("ChatGPT 登录（官号 · {email}）"),
            None => "ChatGPT 登录（官号）".to_string(),
        })
    }

    match tool {
        "claude-code" => {
            let v = read_backup_json(backup_path, "settings.json");
            let env = v.as_ref().and_then(|v| v.get("env")).and_then(|v| v.as_object());
            let base = env_string(env, &["ANTHROPIC_BASE_URL", "ANTHROPIC_API_BASE_URL"]);
            let model = env_string(
                env,
                &[
                    "ANTHROPIC_MODEL",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
                ],
            );
            let key = env_string(env, &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]);
            (base, model, mask_opt(key))
        }
        "codex" => {
            let doc: toml_edit::DocumentMut = read_backup_text(backup_path, "config.toml")
                .unwrap_or_default()
                .parse()
                .unwrap_or_default();
            let model = doc.get("model").and_then(|v| v.as_str()).map(String::from);
            let provider = doc.get("model_provider").and_then(|v| v.as_str());
            let base = codex_provider_base(&doc, provider);
            let auth = read_backup_json(backup_path, "auth.json");
            let key = auth
                .as_ref()
                .and_then(|v| v.get("OPENAI_API_KEY"))
                .and_then(|k| k.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(String::from);
            let key_hint = mask_opt(key).or_else(|| {
                chatgpt_login_hint(auth.as_ref())
            });
            (base, model, key_hint)
        }
        "gemini-cli" => {
            let env = read_backup_text(backup_path, ".env").unwrap_or_default();
            let settings = read_backup_json(backup_path, "settings.json");
            let model = settings
                .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from));
            (
                env_value(&env, "GOOGLE_GEMINI_BASE_URL").or_else(|| env_value(&env, "OPENAI_BASE_URL")),
                model,
                mask_opt(
                    env_value(&env, "GEMINI_API_KEY")
                        .or_else(|| env_value(&env, "GOOGLE_API_KEY"))
                        .or_else(|| env_value(&env, "OPENAI_API_KEY")),
                ),
            )
        }
        "opencode" => {
            let v = read_backup_json(backup_path, "opencode.json");
            let selected_model = v
                .as_ref()
                .and_then(|v| v.get("model"))
                .and_then(|m| m.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(String::from);
            let provider_id = selected_model
                .as_deref()
                .and_then(|m| m.split_once('/').map(|(provider, _)| provider));
            let provider = v
                .as_ref()
                .and_then(|v| opencode_provider(v, provider_id));
            let base = provider.and_then(|p| json_string(p, &["options", "baseURL"]));
            let key = provider.and_then(|p| json_string(p, &["options", "apiKey"]));
            let model = selected_model.or_else(|| {
                provider
                    .and_then(|p| p.get("models"))
                    .and_then(|m| m.as_object())
                    .and_then(|models| models.keys().next().cloned())
            });
            (base, model, mask_opt(key))
        }
        "openclaw" => {
            let v = read_backup_json(backup_path, "config.json");
            let env = v.as_ref().and_then(|v| v.get("env")).and_then(|v| v.as_object());
            let base = env_string(env, &["ANTHROPIC_BASE_URL", "ANTHROPIC_API_BASE_URL"]);
            let model = env_string(
                env,
                &[
                    "ANTHROPIC_MODEL",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
                    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
                ],
            );
            let key = env_string(env, &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]);
            (base, model, mask_opt(key))
        }
        "hermes" => {
            let text = read_backup_text(backup_path, "config.yaml").unwrap_or_default();
            (
                yaml_value(&text, "base_url"),
                yaml_value(&text, "model").or_else(|| yaml_value(&text, "default")),
                mask_opt(yaml_value(&text, "api_key")),
            )
        }
        _ => (None, None, None),
    }
}

fn backup_fingerprint(entry: &BackupEntry) -> String {
    fn norm(value: &Option<String>) -> String {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("—")
            .to_ascii_lowercase()
    }

    format!(
        "{}\n{}\n{}\n{}\n{}",
        entry.tool,
        norm(&entry.base_url),
        norm(&entry.model),
        norm(&entry.key_hint),
        entry.files.join("|"),
    )
}

pub fn list_backups(tool_filter: Option<&str>) -> Result<Vec<BackupEntry>> {
    let home = home_dir()?;
    let root = home.join(".beeapi-switch").join("backups");
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }

    for tool_entry in std::fs::read_dir(&root)
        .with_context(|| format!("Failed to read {}", root.display()))?
        .flatten()
    {
        let tool_path = tool_entry.path();
        if !tool_path.is_dir() {
            continue;
        }
        let tool = tool_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if tool_filter.is_some_and(|filter| filter != tool) {
            continue;
        }
        for backup_entry in std::fs::read_dir(&tool_path)
            .with_context(|| format!("Failed to read {}", tool_path.display()))?
            .flatten()
        {
            let backup_path = backup_entry.path();
            if !backup_path.is_dir() {
                continue;
            }
            let meta = std::fs::metadata(&backup_path).ok();
            let modified = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let files = collect_backup_files(&backup_path);
            let name = backup_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("backup")
                .to_string();
            let (base_url, model, key_hint) = backup_summary(&tool, &backup_path);
            out.push(BackupEntry {
                tool: tool.clone(),
                name,
                path: backup_path.display().to_string(),
                modified,
                files,
                base_url,
                model,
                key_hint,
            });
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    let mut seen = HashSet::new();
    out.retain(|entry| seen.insert(backup_fingerprint(entry)));
    Ok(out)
}

fn backup_root() -> Result<PathBuf> {
    Ok(home_dir()?.join(".beeapi-switch").join("backups"))
}

fn ensure_backup_target(path: &Path) -> Result<PathBuf> {
    let root = backup_root()?;
    let root_canon = root
        .canonicalize()
        .with_context(|| format!("Backup root does not exist: {}", root.display()))?;
    let target_canon = path
        .canonicalize()
        .with_context(|| format!("Backup path does not exist: {}", path.display()))?;

    if !target_canon.starts_with(&root_canon) || target_canon == root_canon {
        return Err(anyhow!(
            "Refusing to modify path outside backup root: {}",
            path.display()
        ));
    }
    if !target_canon.is_dir() {
        return Err(anyhow!("Backup path is not a directory: {}", path.display()));
    }
    Ok(target_canon)
}

fn ensure_backup_target_for_tool(path: &Path, tool: &str) -> Result<PathBuf> {
    if !ALL_TOOLS.contains(&tool) {
        return Err(anyhow!("Unknown tool: {tool}"));
    }
    let target = ensure_backup_target(path)?;
    let root = backup_root()?;
    let tool_root = root.join(tool);
    let tool_root = tool_root
        .canonicalize()
        .with_context(|| format!("Backup path does not exist: {}", tool_root.display()))?;
    if !target.starts_with(&tool_root) {
        return Err(anyhow!(
            "Backup does not belong to {}: {}",
            tool,
            path.display()
        ));
    }
    Ok(target)
}

pub fn delete_backup(path: &str) -> Result<()> {
    let target = ensure_backup_target(&PathBuf::from(path))?;
    std::fs::remove_dir_all(&target)
        .with_context(|| format!("Failed to delete backup {}", target.display()))?;
    Ok(())
}

pub fn clear_backups(tool: &str) -> Result<usize> {
    if !ALL_TOOLS.contains(&tool) {
        return Err(anyhow!("Unknown tool: {tool}"));
    }

    let root = backup_root()?;
    let tool_dir = root.join(tool);
    if !tool_dir.exists() {
        return Ok(0);
    }
    let target = ensure_backup_target(&tool_dir)?;

    let count = std::fs::read_dir(&target)
        .map(|entries| entries.flatten().filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);
    std::fs::remove_dir_all(&target)
        .with_context(|| format!("Failed to clear backups {}", target.display()))?;
    Ok(count)
}

pub fn restore_backup(req: RestoreBackupRequest) -> Result<ApplyResult> {
    let home = home_dir()?;
    let backup_dir = ensure_backup_target_for_tool(&PathBuf::from(&req.path), &req.tool)?;

    let paths = tool_paths(&req.tool, &home)?;
    let backup = backup_existing(&req.tool, &paths)?;
    let mut restored = Vec::new();
    for target in paths {
        let Some(file_name) = target.file_name() else {
            continue;
        };
        let src = backup_dir.join(file_name);
        if !src.exists() || !src.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }
        std::fs::copy(&src, &target).with_context(|| {
            format!("Failed to restore {} -> {}", src.display(), target.display())
        })?;
        restored.push(target.display().to_string());
    }
    if restored.is_empty() {
        return Err(anyhow!(
            "This backup has no files restorable for {}",
            req.tool
        ));
    }
    Ok(ApplyResult {
        tool: req.tool,
        files: restored,
        backup_dir: backup.map(|p| p.display().to_string()),
    })
}

pub fn latest_backup(tool: &str) -> Result<Option<BackupEntry>> {
    let backups = list_backups(Some(tool))?;
    Ok(backups.into_iter().next())
}

pub fn rollback_backup(tool: &str) -> Result<ApplyResult> {
    let backup = latest_backup(tool)?
        .ok_or_else(|| anyhow!("No backup available for {tool}"))?;
    restore_backup(RestoreBackupRequest {
        tool: tool.to_string(),
        path: backup.path,
    })
}

/// Mask a key: show first 4 and last 4 chars, replace middle with ****
fn mask_key(key: &str) -> String {
    let len = key.len();
    if len <= 8 {
        return "****".to_string();
    }
    format!("{}****{}", &key[..4], &key[len - 4..])
}

fn display_key_hint(key: &str) -> String {
    if key.to_ascii_lowercase().contains("chatgpt") {
        key.to_string()
    } else {
        mask_key(key)
    }
}
