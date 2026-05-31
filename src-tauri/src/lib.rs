mod config;
mod detect;
mod models;
mod pool_store;
mod proxy;
mod tools;
mod usage;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::{Duration, Instant},
};

use tauri::{Emitter, Manager};

use crate::config::{ApplyResult, BackupEntry, ConfigStatus, RestoreBackupRequest};
use crate::models::ModelInfo;
use crate::pool_store::StoredSettings;
use crate::proxy::{ApiKey, KeyStat, ProxyHandle, RequestLogEntry, StartOptions};
use crate::tools::codex_root;

static PROXY: OnceLock<Arc<ProxyHandle>> = OnceLock::new();
static PRIMARY_KEY_ID: OnceLock<RwLock<Option<String>>> = OnceLock::new();
static RECENT_DEEP_LINK_URLS: OnceLock<Mutex<VecDeque<(String, Instant)>>> = OnceLock::new();
static PENDING_IMPORT_NOTICE: OnceLock<Mutex<Option<serde_json::Value>>> = OnceLock::new();
const DEEP_LINK_SCHEMES: &[&str] = &["beeapi-switch", "beeapi"];

fn proxy() -> Arc<ProxyHandle> {
    match PROXY.get().cloned() {
        Some(handle) => handle,
        None => panic!("proxy handle not initialized"),
    }
}

fn primary_slot() -> &'static RwLock<Option<String>> {
    PRIMARY_KEY_ID.get_or_init(|| RwLock::new(None))
}

fn set_primary_key(id: Option<String>) {
    if let Ok(mut guard) = primary_slot().write() {
        *guard = id;
    }
}

fn get_primary_key() -> Option<String> {
    primary_slot().read().ok().and_then(|guard| guard.clone())
}

/// Return the primary key (for direct-mode / fetch-models). Prefers the
/// explicit primary ID, falls back to first enabled.
fn primary_secret() -> Option<String> {
    let keys = proxy().pool.raw_keys();
    if let Some(id) = get_primary_key() {
        if let Some(k) = keys.iter().find(|k| k.id == id && k.enabled) {
            return Some(k.secret.clone());
        }
    }
    keys.into_iter().find(|k| k.enabled).map(|k| k.secret)
}

fn allowed_session_roots(home: &std::path::Path) -> Vec<std::path::PathBuf> {
    vec![home.join(".claude").join("projects"), codex_root(&home.to_path_buf()).join("sessions")]
}

fn ensure_session_target(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("cannot locate home directory")?;
    let target = path
        .canonicalize()
        .map_err(|e| format!("session file does not exist or is not accessible: {} ({e})", path.display()))?;
    let allowed = allowed_session_roots(&home)
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect::<Vec<_>>();
    if allowed.iter().any(|root| target.starts_with(root)) {
        Ok(target)
    } else {
        Err(format!("refusing to operate on session path outside allowed roots: {}", path.display()))
    }
}

fn persist_settings() -> Result<(), String> {
    let p = proxy();
    let usage = p.pool.token_usage();
    let settings = StoredSettings {
        keys: p.pool.raw_keys(),
        upstream: Some(p.upstream()),
        local_token: Some(p.pool.local_token()),
        lan: Some(p.pool.lan()),
        pool_enabled: Some(p.pool.pool_enabled()),
        primary_key_id: get_primary_key(),
        fail_threshold: Some(p.pool.fail_threshold()),
        total_input_tokens: Some(usage.total_input),
        total_output_tokens: Some(usage.total_output),
        total_cache_read_tokens: Some(usage.total_cache_read),
        total_cache_write_tokens: Some(usage.total_cache_write),
    };
    pool_store::save(&settings).map_err(|e| format!("{e:#}"))
}

// ----------------------------------------------------------- deep link import

#[derive(Clone, Debug, serde::Serialize)]
struct ImportUrlPayload {
    keys_added: usize,
    duplicates: usize,
    upstream: Option<String>,
    model: Option<String>,
    pool_enabled: Option<bool>,
    primary_key_id: Option<String>,
}

fn import_key_label(index: usize) -> String {
    format!("imported-{index}")
}

fn normalize_import_secret(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

fn is_probable_api_key(secret: &str) -> bool {
    let len = secret.chars().count();
    len >= 16
        && len <= 4096
        && secret
            .chars()
            .all(|c| !c.is_control() && !c.is_whitespace())
}

fn append_import_keys(raw_keys: Vec<String>, label_prefix: Option<String>) -> (usize, usize, Option<String>) {
    let p = proxy();
    let mut keys = p.pool.raw_keys();
    let mut seen = keys
        .iter()
        .map(|k| k.secret.trim().to_string())
        .collect::<std::collections::HashSet<_>>();
    let mut added = 0usize;
    let mut duplicates = 0usize;
    let mut primary_id = None;

    for raw in raw_keys {
        let secret = normalize_import_secret(&raw);
        if !is_probable_api_key(&secret) {
            continue;
        }
        if !seen.insert(secret.clone()) {
            duplicates += 1;
            continue;
        }
        let id = proxy::random_token();
        if primary_id.is_none() {
            primary_id = Some(id.clone());
        }
        let label = label_prefix
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                if added == 0 {
                    s.trim().to_string()
                } else {
                    format!("{}-{}", s.trim(), added + 1)
                }
            })
            .unwrap_or_else(|| import_key_label(keys.len() + 1));
        keys.push(ApiKey {
            id,
            secret,
            label,
            enabled: true,
            weight: 1,
        });
        added += 1;
    }

    if added > 0 {
        p.pool.set_keys(keys);
    }
    (added, duplicates, primary_id)
}

fn parse_import_url(raw_url: &str) -> Result<(Vec<String>, Option<String>, Option<String>, Option<bool>), String> {
    let url = url::Url::parse(raw_url).map_err(|e| format!("无法解析导入链接: {e}"))?;
    let scheme = url.scheme();
    if !DEEP_LINK_SCHEMES.contains(&scheme) {
        return Err(format!("不支持的导入协议: {scheme}"));
    }

    let mut keys = Vec::new();
    let mut model = None;
    let mut label = None;
    let mut pool_enabled = None;

    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "key" | "api_key" | "apikey" | "token" | "secret" => {
                keys.push(value.into_owned());
            }
            "keys" | "api_keys" => {
                keys.extend(
                    value
                        .split([',', ';', '\n', '\r', ' '])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                );
            }
            "base" | "base_url" | "api" | "api_url" | "api_address" | "upstream" => {}
            "model" => model = Some(value.into_owned()),
            "label" | "name" | "key_name" | "keyName" | "title" => label = Some(value.into_owned()),
            "pool" | "pool_enabled" => {
                let v = value.to_ascii_lowercase();
                pool_enabled = match v.as_str() {
                    "1" | "true" | "yes" | "on" => Some(true),
                    "0" | "false" | "no" | "off" => Some(false),
                    _ => None,
                };
            }
            _ => {}
        }
    }

    // Also support compact links such as:
    //   beeapi-switch://import/sk-xxx?name=key-1
    //   beeapi://sk-xxx
    let path = url.path().trim_matches('/');
    if !path.is_empty() && path != "import" && path != "add" {
        if let Some(last) = path.rsplit('/').next() {
            if last != "import" && last != "add" {
                keys.push(last.to_string());
            }
        }
    }
    if let Some(host) = url.host_str() {
        if host != "import" && host != "add" {
            keys.push(host.to_string());
        }
    }

    Ok((keys, model, label, pool_enabled))
}

fn import_from_url(raw_url: &str) -> Result<ImportUrlPayload, String> {
    let (keys, model, label, pool_enabled) = parse_import_url(raw_url)?;
    let (keys_added, duplicates, first_added_id) = append_import_keys(keys, label);
    let p = proxy();

    if let Some(enabled) = pool_enabled {
        p.pool.set_pool_enabled(enabled);
    }
    if let Some(id) = first_added_id.clone() {
        if get_primary_key().is_none() {
            p.pool.set_preferred_key_id(Some(id.clone()));
            set_primary_key(Some(id));
        }
    }

    persist_settings()?;
    Ok(ImportUrlPayload {
        keys_added,
        duplicates,
        upstream: None,
        model,
        pool_enabled,
        primary_key_id: first_added_id,
    })
}

fn should_process_deep_link_url(url: &str) -> bool {
    let recent = RECENT_DEEP_LINK_URLS.get_or_init(|| Mutex::new(VecDeque::new()));
    let Ok(mut guard) = recent.lock() else {
        return true;
    };
    let now = Instant::now();
    while let Some((_, ts)) = guard.front() {
        if now.duration_since(*ts) > Duration::from_secs(30) {
            guard.pop_front();
        } else {
            break;
        }
    }
    if guard.iter().any(|(seen, _)| seen == url) {
        return false;
    }
    guard.push_back((url.to_string(), now));
    true
}

fn pending_import_slot() -> &'static Mutex<Option<serde_json::Value>> {
    PENDING_IMPORT_NOTICE.get_or_init(|| Mutex::new(None))
}

fn store_import_success(payload: &ImportUrlPayload) {
    if let Ok(mut guard) = pending_import_slot().lock() {
        *guard = Some(serde_json::json!({
            "ok": true,
            "payload": payload,
        }));
    }
}

fn store_import_error(err: &str) {
    if let Ok(mut guard) = pending_import_slot().lock() {
        *guard = Some(serde_json::json!({
            "ok": false,
            "error": err,
        }));
    }
}

#[tauri::command]
fn import_from_deep_link(url: String) -> Result<ImportUrlPayload, String> {
    import_from_url(&url)
}

#[tauri::command]
fn take_pending_import_notice() -> Option<serde_json::Value> {
    pending_import_slot().lock().ok().and_then(|mut guard| guard.take())
}

fn handle_deep_link_urls(app: &tauri::AppHandle, urls: Vec<String>) {
    for url in urls {
        if !should_process_deep_link_url(&url) {
            continue;
        }
        match import_from_url(&url) {
            Ok(payload) => {
                store_import_success(&payload);
                let _ = app.emit("beeapi-imported", payload);
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
            Err(err) => {
                store_import_error(&err);
                let _ = app.emit("beeapi-import-error", err);
            }
        }
    }
}

fn collect_startup_deep_link_urls() -> Vec<String> {
    std::env::args()
        .filter(|arg| DEEP_LINK_SCHEMES.iter().any(|scheme| arg.starts_with(&format!("{scheme}:"))))
        .collect()
}

#[derive(serde::Serialize)]
struct DeepLinkRegistrationResult {
    schemes: Vec<String>,
    exe: String,
}

#[tauri::command]
fn register_deep_link_service() -> Result<DeepLinkRegistrationResult, String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;

        let exe = std::env::current_exe()
            .map_err(|e| format!("无法定位当前程序路径: {e}"))?;
        let exe_cmd = format!("\"{}\" \"%1\"", exe.display());
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (classes, _) = hkcu
            .create_subkey("Software\\Classes")
            .map_err(|e| format!("无法打开注册表: {e}"))?;

        for scheme in DEEP_LINK_SCHEMES {
            let (key, _) = classes
                .create_subkey(scheme)
                .map_err(|e| format!("无法创建协议项 {scheme}: {e}"))?;
            key.set_value("", &format!("URL:{scheme} Protocol"))
                .map_err(|e| format!("无法写入协议描述 {scheme}: {e}"))?;
            key.set_value("URL Protocol", &"")
                .map_err(|e| format!("无法写入协议标记 {scheme}: {e}"))?;
            let (shell, _) = key
                .create_subkey("shell\\open\\command")
                .map_err(|e| format!("无法创建打开命令 {scheme}: {e}"))?;
            shell
                .set_value("", &exe_cmd)
                .map_err(|e| format!("无法写入打开命令 {scheme}: {e}"))?;
        }

        return Ok(DeepLinkRegistrationResult {
            schemes: DEEP_LINK_SCHEMES.iter().map(|s| s.to_string()).collect(),
            exe: exe.display().to_string(),
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("当前平台暂不支持手动注册超链服务".to_string())
    }
}

// ----------------------------------------------------------- config writers

#[tauri::command]
fn apply_config(tool: String, model: String) -> Result<ApplyResult, String> {
    let p = proxy();
    if p.pool.pool_enabled() {
        // Route via the local proxy: CLI uses 127.0.0.1 + local token.
        let local_base = p.local_base_for_cli();
        let token = p.pool.local_token();
        let result = config::apply(&tool, &token, &model, Some(&local_base))
            .map_err(|e| format!("{e:#}"))?;
        Ok(result)
    } else {
        // Direct mode: the CLI talks to the upstream with the primary key.
        let secret = primary_secret().ok_or_else(|| {
            "池子已关闭，直连模式下需要池中至少一把启用的密钥作为主密钥".to_string()
        })?;
        let result = config::apply(&tool, &secret, &model, Some(&p.upstream()))
            .map_err(|e| format!("{e:#}"))?;
        Ok(result)
    }
}

#[tauri::command]
fn clear_config(tool: String) -> Result<(), String> {
    config::clear(&tool).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn list_status() -> Result<Vec<ConfigStatus>, String> {
    config::list_status().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn sync_codex_history() -> Result<serde_json::Value, String> {
    let home = dirs::home_dir().ok_or_else(|| "cannot locate home directory".to_string())?;
    let codex_dir = codex_root(&home);
    if !codex_dir.exists() {
        return Err("Codex home directory not found (~/.codex)".to_string());
    }

    let config_path = codex_dir.join("config.toml");
    let mut target_provider = "openai".to_string();
    if config_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&config_path) {
            let doc: toml_edit::DocumentMut = text.parse().unwrap_or_default();
            if let Some(prov) = doc.get("model_provider").and_then(|v| v.as_str()) {
                if !prov.trim().is_empty() {
                    target_provider = prov.to_string();
                }
            }
        }
    }

    let mut backup_dir: Option<std::path::PathBuf> = None;
    let ensure_backup_dir = |slot: &mut Option<std::path::PathBuf>| -> Result<std::path::PathBuf, String> {
        if let Some(dir) = slot {
            return Ok(dir.clone());
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let dir = home
            .join(".beeapi-switch")
            .join("history-backups")
            .join("codex-history")
            .join(stamp);
        std::fs::create_dir_all(dir.join("files"))
            .map_err(|e| format!("failed to create history backup directory: {e}"))?;
        *slot = Some(dir.clone());
        Ok(dir)
    };
    let backup_file = |backup_root: &std::path::Path, src: &std::path::Path| -> Result<(), String> {
        let rel = src
            .strip_prefix(&codex_dir)
            .map_err(|e| format!("failed to create history backup directory: {e}"))?;
        let dst = backup_root.join("files").join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create backup subdirectory: {e}"))?;
        }
        if !dst.exists() {
            std::fs::copy(src, &dst)
                .map_err(|e| format!("failed to backup {}: {e}", src.display()))?;
        }
        Ok(())
    };

    let mut files_updated = 0;
    let session_dirs = ["sessions", "archived_sessions"];
    for sub_dir in session_dirs {
        let dir_path = codex_dir.join(sub_dir);
        if dir_path.exists() {
            let mut files = Vec::new();
            fn walk_dir(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            walk_dir(&path, out);
                        } else if path.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                            .unwrap_or(false)
                        {
                            out.push(path);
                        }
                    }
                }
            }
            walk_dir(&dir_path, &mut files);

            for path in files {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                    if lines.is_empty() {
                        continue;
                    }
                    if let Ok(mut record) = serde_json::from_str::<serde_json::Value>(&lines[0]) {
                        if let Some(payload) = record.get_mut("payload").and_then(|p| p.as_object_mut()) {
                            let current_prov = payload.get("model_provider").and_then(|p| p.as_str());
                            if current_prov != Some(&target_provider) {
                                let dir = ensure_backup_dir(&mut backup_dir)?;
                                backup_file(&dir, &path)?;
                                payload.insert("model_provider".to_string(), serde_json::json!(target_provider));
                                if let Ok(new_first_line) = serde_json::to_string(&record) {
                                    lines[0] = new_first_line;
                                    let new_content = lines.join("\n") + "\n";
                                    std::fs::write(&path, new_content)
                                        .map_err(|e| format!("failed to write session file {}: {e}", path.display()))?;
                                    files_updated += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let db_path = codex_dir.join("state_5.sqlite");
    let mut db_updated = 0;
    if db_path.exists() {
        let dir = ensure_backup_dir(&mut backup_dir)?;
        std::fs::create_dir_all(dir.join("db"))
            .map_err(|e| format!("failed to create database backup directory: {e}"))?;
        let db_backup = dir.join("db").join("state_5.sqlite");
        if !db_backup.exists() {
            std::fs::copy(&db_path, &db_backup)
                .map_err(|e| format!("failed to backup/restore Codex database: {e}"))?;
        }
        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
            let run_update = conn.execute(
                "UPDATE threads SET model_provider = ?1 WHERE COALESCE(model_provider, '') <> ?1",
                [&target_provider],
            );
            if let Ok(rows) = run_update {
                db_updated = rows;
            }
        }
    }

    Ok(serde_json::json!({
        "files_updated": files_updated,
        "db_updated": db_updated,
        "provider": target_provider,
        "backup_dir": backup_dir.map(|p| p.display().to_string()),
    }))
}

fn copy_dir_contents(src_root: &std::path::Path, dst_root: &std::path::Path) -> Result<usize, String> {
    fn walk(src_root: &std::path::Path, dst_root: &std::path::Path, dir: &std::path::Path, count: &mut usize) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("failed to read or restore backup: {e}"))? {
            let entry = entry.map_err(|e| format!("failed to read or restore backup: {e}"))?;
            let path = entry.path();
            if path.is_dir() {
                walk(src_root, dst_root, &path, count)?;
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(src_root)
                    .map_err(|e| format!("failed to read or restore backup: {e}"))?;
                let dst = dst_root.join(rel);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("failed to read or restore backup: {e}"))?;
                }
                std::fs::copy(&path, &dst)
                    .map_err(|e| format!("failed to backup {}: {e}", dst.display()))?;
                *count += 1;
            }
        }
        Ok(())
    }
    let mut count = 0;
    if src_root.exists() {
        walk(src_root, dst_root, src_root, &mut count)?;
    }
    Ok(count)
}

#[tauri::command]
fn rollback_latest_codex_history_backup() -> Result<serde_json::Value, String> {
    let home = dirs::home_dir().ok_or_else(|| "cannot locate home directory".to_string())?;
    let codex_dir = codex_root(&home);
    let root = home
        .join(".beeapi-switch")
        .join("history-backups")
        .join("codex-history");
    if !root.exists() {
        return Err("no Codex history backup found".to_string());
    }
    let mut backups = std::fs::read_dir(&root)
        .map_err(|e| format!("failed to create history backup directory: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect::<Vec<_>>();
    backups.sort();
    let backup = backups
        .pop()
        .ok_or_else(|| "no Codex history backup found".to_string())?;

    let files_restored = copy_dir_contents(&backup.join("files"), &codex_dir)?;
    let db_backup = backup.join("db").join("state_5.sqlite");
    let mut db_restored = false;
    if db_backup.exists() {
        let db_path = codex_dir.join("state_5.sqlite");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create Codex directory: {e}"))?;
        }
        std::fs::copy(&db_backup, &db_path)
            .map_err(|e| format!("failed to backup/restore Codex database: {e}"))?;
        db_restored = true;
    }

    Ok(serde_json::json!({
        "backup_dir": backup.display().to_string(),
        "files_restored": files_restored,
        "db_restored": db_restored,
    }))
}

#[tauri::command]
async fn download_image_to(url: String, path: String) -> Result<(), String> {
    let parsed = url::Url::parse(&url).map_err(|e| format!("invalid image URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http/https image URLs are allowed".to_string());
    }

    let target = std::path::PathBuf::from(&path);
    let ext = target
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
        return Err("image save path must end with .png/.jpg/.jpeg/.webp".to_string());
    }
    let parent = target
        .parent()
        .ok_or_else(|| "save path has no parent directory".to_string())?;
    if !parent.exists() || !parent.is_dir() {
        return Err("save directory does not exist".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| format!("failed to create download client: {e:#}"))?;

    let resp = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| format!("image URL request failed: {e:#}"))?;

    if !resp.status().is_success() {
        return Err(format!("image download failed with HTTP status: {}", resp.status()));
    }
    if let Some(content_type) = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        if !content_type.to_ascii_lowercase().starts_with("image/") {
            return Err(format!("downloaded content is not an image: {content_type}"));
        }
    }
    if let Some(len) = resp.content_length() {
        if len > 25 * 1024 * 1024 {
            return Err("image is larger than 25MB".to_string());
        }
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed to read image bytes: {e:#}"))?;
    if bytes.len() > 25 * 1024 * 1024 {
        return Err("image is larger than 25MB".to_string());
    }

    std::fs::write(&target, &bytes)
        .map_err(|e| format!("failed to write image to disk: {e:#}"))?;

    Ok(())
}

// ----------------------------------------------------------- model catalog

#[tauri::command]
async fn fetch_models(key_id: Option<String>) -> Result<Vec<ModelInfo>, String> {
    let p = proxy();
    if p.pool.pool_enabled() {
        // Route through the local proxy so the request reuses the round-robin
        // retry logic and any cool-down state. We call the same endpoint the
        // CLI would hit.
        let base = p.local_base_for_cli();
        let token = p.pool.local_token();
        models::fetch(&base, &token, key_id.as_deref())
            .await
            .map_err(|e| format!("{e:#}"))
    } else {
        // Direct mode: if key_id is specified, use its secret.
        let secret = if let Some(ref id) = key_id {
            let keys = p.pool.raw_keys();
            keys.into_iter()
                .find(|k| &k.id == id && k.enabled)
                .map(|k| k.secret)
                .ok_or_else(|| "选中的密钥未启用或不存在".to_string())?
        } else {
            primary_secret().ok_or_else(|| "请在密钥池中添加至少一把启用的 API Key".to_string())?
        };
        models::fetch(&p.upstream(), &secret, None)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

// ----------------------------------------------------------- proxy info

/// Quick health check — just confirms the proxy thread is alive and listening.
#[tauri::command]
async fn proxy_health() -> Result<bool, String> {
    let p = proxy();
    let url = format!("{}/__beeapi/health", p.local_base_for_cli());
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    match client.get(&url).send().await {
        Ok(resp) => Ok(resp.status().is_success()),
        Err(_) => Ok(false),
    }
}

#[derive(serde::Serialize)]
struct ProxyInfo {
    local_base: String,
    lan_base: Option<String>,
    upstream: String,
    port: u16,
    token: String,
    lan: bool,
    pool_enabled: bool,
    primary_key_id: Option<String>,
    fail_threshold: u32,
}

#[tauri::command]
fn proxy_info() -> ProxyInfo {
    let p = proxy();
    ProxyInfo {
        local_base: p.local_base_for_cli(),
        lan_base: p.lan_base(),
        upstream: p.upstream(),
        port: p.port,
        token: p.pool.local_token(),
        lan: p.pool.lan(),
        pool_enabled: p.pool.pool_enabled(),
        primary_key_id: get_primary_key(),
        fail_threshold: p.pool.fail_threshold(),
    }
}

#[tauri::command]
fn set_lan(enabled: bool) -> Result<ProxyInfo, String> {
    proxy().pool.set_lan(enabled);
    persist_settings()?;
    Ok(proxy_info())
}

#[tauri::command]
fn set_pool_enabled(enabled: bool) -> Result<ProxyInfo, String> {
    proxy().pool.set_pool_enabled(enabled);
    persist_settings()?;
    Ok(proxy_info())
}

#[tauri::command]
fn set_primary_key_id(id: Option<String>) -> Result<(), String> {
    proxy().pool.set_preferred_key_id(id.clone());
    set_primary_key(id);
    persist_settings()
}

#[tauri::command]
fn regenerate_token() -> Result<String, String> {
    let t = proxy::random_token();
    proxy().pool.set_local_token(t.clone());
    persist_settings()?;
    Ok(t)
}

#[tauri::command]
fn set_fail_threshold(threshold: u32) -> Result<u32, String> {
    let p = proxy();
    p.pool.set_fail_threshold(threshold);
    persist_settings()?;
    Ok(p.pool.fail_threshold())
}

// ----------------------------------------------------------- pool mgmt

#[tauri::command]
fn pool_stats() -> Vec<KeyStat> {
    proxy().pool.stats()
}

#[tauri::command]
fn token_usage() -> serde_json::Value {
    let usage = proxy().pool.token_usage();
    serde_json::json!({
        "session_input": usage.session_input,
        "session_output": usage.session_output,
        "session_cache_read": usage.session_cache_read,
        "session_cache_write": usage.session_cache_write,
        "total_input": usage.total_input,
        "total_output": usage.total_output,
        "total_cache_read": usage.total_cache_read,
        "total_cache_write": usage.total_cache_write,
    })
}

#[tauri::command]
fn reset_token_usage() -> Result<(), String> {
    proxy().pool.reset_usage();
    persist_settings()
}

fn openai_v1_root(base: &str) -> String {
    let root = base.trim().trim_end_matches('/');
    if root.ends_with("/v1") {
        root.to_string()
    } else {
        format!("{root}/v1")
    }
}

/// Verify a key by calling /v1/models. Returns Ok(model_count) or Err(reason).
#[tauri::command]
async fn verify_key(secret: String) -> Result<usize, String> {
    let p = proxy();
    let upstream = p.upstream();
    let url = format!("{}/models", openai_v1_root(&upstream));
    let client = reqwest::Client::builder()
        .user_agent("beeapi-switch/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP 客户端错误: {e}"))?;
    let resp = client
        .get(&url)
        .bearer_auth(secret.trim())
        .send()
        .await
        .map_err(|e| humanize_network_error(&e))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(humanize_status(status));
    }
    let v: serde_json::Value = resp.json().await.map_err(|_| "响应格式异常".to_string())?;
    let count = v
        .get("data")
        .and_then(|d| d.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Ok(count)
}

fn humanize_network_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "连接超时，请检查网络或上游地址".to_string();
    }
    if e.is_connect() {
        return "无法连接到上游服务器，请检查网络".to_string();
    }
    format!("网络错误: {e}")
}

fn humanize_status(status: u16) -> String {
    match status {
        401 => "密钥无效或已过期 (401)".to_string(),
        402 => "余额不足 (402)".to_string(),
        403 => "访问被拒绝，密钥权限不足 (403)".to_string(),
        404 => "接口不存在，请检查上游地址 (404)".to_string(),
        429 => "请求过于频繁，请稍后再试 (429)".to_string(),
        500 => "上游服务器内部错误 (500)".to_string(),
        502 => "上游网关错误 (502)".to_string(),
        503 => "上游服务暂时不可用 (503)".to_string(),
        _ => format!("上游返回错误 ({status})"),
    }
}

#[tauri::command]
fn set_pool_keys(keys: Vec<ApiKey>) -> Result<(), String> {
    proxy().pool.set_keys(keys);
    persist_settings()
}

#[tauri::command]
fn load_pool() -> serde_json::Value {
    let p = proxy();
    serde_json::json!({
        "keys": p.pool.raw_keys(),
        "upstream": p.upstream(),
        "lan": p.pool.lan(),
        "pool_enabled": p.pool.pool_enabled(),
        "primary_key_id": get_primary_key(),
        "fail_threshold": p.pool.fail_threshold(),
    })
}

// ----------------------------------------------------------- sessions

#[tauri::command]
fn request_log() -> Vec<RequestLogEntry> {
    proxy().pool.recent_log()
}

#[tauri::command]
fn clear_request_log() {
    proxy().pool.clear_log();
}


#[tauri::command]
fn list_config_backups(tool: Option<String>) -> Result<Vec<BackupEntry>, String> {
    config::list_backups(tool.as_deref()).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn restore_config_backup(tool: String, path: String) -> Result<ApplyResult, String> {
    config::restore_backup(RestoreBackupRequest { tool, path }).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn rollback_config_backup(tool: String) -> Result<ApplyResult, String> {
    config::rollback_backup(&tool).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn delete_config_backup(path: String) -> Result<(), String> {
    config::delete_backup(&path).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn clear_config_backups(tool: String) -> Result<usize, String> {
    config::clear_backups(&tool).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn export_request_log_csv() -> String {
    fn esc(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
    let mut out = String::from("id,time,method,path,model,status,key_label,key_id,latency_ms,retries,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,usage_recorded,upstream_error\n");
    for e in proxy().pool.recent_log() {
        let row = [
            e.id.to_string(),
            e.ts.to_string(),
            e.method,
            e.path,
            e.model.unwrap_or_default(),
            e.status.to_string(),
            e.key_label,
            e.key_id,
            e.latency_ms.to_string(),
            e.retries.to_string(),
            e.input_tokens.to_string(),
            e.output_tokens.to_string(),
            e.cache_read_tokens.to_string(),
            e.cache_write_tokens.to_string(),
            e.usage_recorded.to_string(),
            e.upstream_error.unwrap_or_default(),
        ];
        out.push_str(&row.iter().map(|v| esc(v)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

#[tauri::command]
fn export_usage_csv() -> Result<String, String> {
    fn esc(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
    let snapshot = usage::scan().map_err(|e| format!("{e:#}"))?;
    let mut out = String::from("time,tool,session,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,total_tokens,path\n");
    for r in snapshot.records {
        let total = r.input_tokens + r.output_tokens + r.cache_read_tokens + r.cache_write_tokens;
        let row = [
            r.ts.to_string(),
            r.tool,
            r.session,
            r.model,
            r.input_tokens.to_string(),
            r.output_tokens.to_string(),
            r.cache_read_tokens.to_string(),
            r.cache_write_tokens.to_string(),
            total.to_string(),
            r.path,
        ];
        out.push_str(&row.iter().map(|v| esc(v)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    Ok(out)
}

// ----------------------------------------------------------- usage query

#[derive(serde::Serialize)]
struct UsageInfo {
    is_valid: bool,
    remaining: Option<f64>,
    unit: String,
    invalid_message: Option<String>,
    total: Option<f64>,
    used: Option<f64>,
    key_name: Option<String>,
    key_prefix: Option<String>,
    plan_name: Option<String>,
    expires_at: Option<String>,
}

fn as_f64_robust(value: Option<&serde_json::Value>) -> Option<f64> {
    let val = value?;
    if let Some(f) = val.as_f64() {
        Some(f)
    } else if let Some(s) = val.as_str() {
        s.parse::<f64>().ok()
    } else if let Some(i) = val.as_i64() {
        Some(i as f64)
    } else if let Some(u) = val.as_u64() {
        Some(u as f64)
    } else {
        None
    }
}

fn as_bool_robust(value: Option<&serde_json::Value>) -> Option<bool> {
    let val = value?;
    if let Some(b) = val.as_bool() {
        Some(b)
    } else if let Some(s) = val.as_str() {
        match s.to_lowercase().as_str() {
            "true" | "1" | "yes" | "ok" | "active" => Some(true),
            "false" | "0" | "no" | "inactive" => Some(false),
            _ => None,
        }
    } else if let Some(i) = val.as_i64() {
        Some(i != 0)
    } else {
        None
    }
}

#[tauri::command]
async fn query_usage() -> Result<UsageInfo, String> {
    let p = proxy();
    // When pool is enabled, route through the local proxy so it can
    // round-robin and auto-retry across keys.
    let (base, key) = if p.pool.pool_enabled() {
        (p.local_base_for_cli(), p.pool.local_token())
    } else {
        let secret =
            primary_secret().ok_or_else(|| "请先添加至少一把启用的 API Key".to_string())?;
        (p.upstream(), secret)
    };
    let url = format!("{}/usage", openai_v1_root(&base));
    let client = reqwest::Client::builder()
        .user_agent("beeapi-switch/0.1")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let resp = client
        .get(&url)
        .bearer_auth(&key)
        .send()
        .await
        .map_err(|e| format!("请求 {url} 失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "{url} 返回 {status}: {}",
            &body[..body.len().min(200)]
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 JSON 失败: {e}"))?;
    let remaining = as_f64_robust(v.get("remaining"))
        .or_else(|| as_f64_robust(v.pointer("/quota/remaining")))
        .or_else(|| as_f64_robust(v.get("balance")));
    let unit = v
        .get("unit")
        .and_then(|u| u.as_str())
        .or_else(|| v.pointer("/quota/unit").and_then(|u| u.as_str()))
        .unwrap_or("USD")
        .to_string();
    let is_valid = as_bool_robust(v.get("is_active"))
        .or_else(|| as_bool_robust(v.get("isValid")))
        .unwrap_or(true);

    let invalid_message = v.get("invalid_message").and_then(|m| m.as_str()).map(String::from);
    let total = as_f64_robust(v.get("total"));
    let used = as_f64_robust(v.get("used"));
    let key_name = v.get("key_name").and_then(|k| k.as_str()).map(String::from);
    let key_prefix = v.get("key_prefix").and_then(|k| k.as_str()).map(String::from);
    let plan_name = v.get("plan_name").and_then(|p| p.as_str()).map(String::from);
    let expires_at = v.get("expires_at").and_then(|e| e.as_str()).map(String::from);

    Ok(UsageInfo {
        is_valid,
        remaining,
        unit,
        invalid_message,
        total,
        used,
        key_name,
        key_prefix,
        plan_name,
        expires_at,
    })
}

#[derive(serde::Serialize)]
struct HealthCheckStep {
    name: String,
    ok: bool,
    detail: String,
}

#[derive(serde::Serialize)]
struct HealthCheckReport {
    tool: String,
    base_url: String,
    model: String,
    ok: bool,
    elapsed_ms: u128,
    steps: Vec<HealthCheckStep>,
}

#[derive(Clone, Copy)]
enum ProbeKind {
    OpenAIResponses,
    OpenAIChat,
    AnthropicMessages,
}

fn probe_kind(tool: &str, wire_api: Option<&str>) -> ProbeKind {
    match tool {
        "codex" => match wire_api.unwrap_or("responses").to_ascii_lowercase().as_str() {
            "chat" | "chat_completions" | "chat-completions" => ProbeKind::OpenAIChat,
            _ => ProbeKind::OpenAIResponses,
        },
        "claude-code" | "openclaw" => ProbeKind::AnthropicMessages,
        _ => ProbeKind::OpenAIChat,
    }
}

fn probe_endpoint(kind: ProbeKind, base: &str) -> String {
    let root = openai_v1_root(base);
    match kind {
        ProbeKind::OpenAIResponses => format!("{root}/responses"),
        ProbeKind::OpenAIChat => format!("{root}/chat/completions"),
        ProbeKind::AnthropicMessages => format!("{root}/messages"),
    }
}

fn probe_headers(kind: ProbeKind, key: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue};

    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", key.trim())) {
        headers.insert(reqwest::header::AUTHORIZATION, v);
    }
    match kind {
        ProbeKind::AnthropicMessages => {
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            if let Ok(v) = HeaderValue::from_str(key.trim()) {
                headers.insert("x-api-key", v);
            }
        }
        ProbeKind::OpenAIResponses | ProbeKind::OpenAIChat => {}
    }
    headers
}

fn probe_body(kind: ProbeKind, model: &str, stream: bool) -> serde_json::Value {
    match kind {
        ProbeKind::OpenAIResponses => serde_json::json!({
            "model": model,
            "input": "ping",
            "max_output_tokens": 1,
            "stream": stream,
        }),
        ProbeKind::OpenAIChat => serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
            "stream": stream,
        }),
        ProbeKind::AnthropicMessages => serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
            "stream": stream,
        }),
    }
}

async fn health_probe(tool: String) -> Result<HealthCheckReport, String> {
    let started = std::time::Instant::now();
    let status = config::tool_status(&tool).map_err(|e| format!("{e:#}"))?;
    let base = status
        .base_url
        .clone()
        .ok_or_else(|| "当前工具还没有写入配置".to_string())?;
    let key = status
        .key
        .clone()
        .ok_or_else(|| "当前配置缺少 API Key".to_string())?;
    let configured_model = status.model.clone();

    let client = reqwest::Client::builder()
        .user_agent("beeapi-switch/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let mut steps = Vec::new();
    let models = match models::fetch(&base, &key, None).await {
        Ok(models) => {
            steps.push(HealthCheckStep {
                name: "模型列表".into(),
                ok: true,
                detail: format!("获取成功，共 {} 个可用模型", models.len()),
            });
            models
        }
        Err(e) => {
            steps.push(HealthCheckStep {
                name: "模型列表".into(),
                ok: false,
                detail: format!("{e:#}"),
            });
            return Ok(HealthCheckReport {
                tool,
                base_url: base,
                model: configured_model.unwrap_or_default(),
                ok: false,
                elapsed_ms: started.elapsed().as_millis(),
                steps,
            });
        }
    };

    let configured_model = configured_model.filter(|m| !m.trim().is_empty());
    let model = if let Some(model) = configured_model {
        if models.iter().any(|m| m.id == model) {
            steps.push(HealthCheckStep {
                name: "模型选择".into(),
                ok: true,
                detail: format!("配置模型可用: {model}"),
            });
            model
        } else {
            steps.push(HealthCheckStep {
                name: "模型选择".into(),
                ok: false,
                detail: format!("配置模型不在当前模型列表中: {model}"),
            });
            return Ok(HealthCheckReport {
                tool,
                base_url: base,
                model,
                ok: false,
                elapsed_ms: started.elapsed().as_millis(),
                steps,
            });
        }
    } else {
        let model = models
            .first()
            .map(|m| m.id.clone())
            .ok_or_else(|| "没有可用于测试的模型".to_string())?;
        steps.push(HealthCheckStep {
            name: "模型选择".into(),
            ok: true,
            detail: format!("未配置模型，使用列表首个模型测试: {model}"),
        });
        model
    };
    let kind = probe_kind(&tool, status.wire_api.as_deref());
    let endpoint = probe_endpoint(kind, &base);
    let headers = probe_headers(kind, &key);

    let non_stream_resp = client
        .post(&endpoint)
        .headers(headers.clone())
        .json(&probe_body(kind, &model, false))
        .send()
        .await
        .map_err(|e| format!("普通请求发送失败: {e}"))?;
    if non_stream_resp.status().is_success() {
        let snippet = non_stream_resp.text().await.unwrap_or_default();
        steps.push(HealthCheckStep {
            name: "普通请求".into(),
            ok: true,
            detail: format!("请求成功，返回 {} 字节", snippet.len()),
        });
    } else {
        let status_code = non_stream_resp.status();
        let body = non_stream_resp.text().await.unwrap_or_default();
        steps.push(HealthCheckStep {
            name: "普通请求".into(),
            ok: false,
            detail: format!("{}: {}", status_code, body.chars().take(200).collect::<String>()),
        });
        return Ok(HealthCheckReport {
            tool,
            base_url: base,
            model,
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            steps,
        });
    }

    let stream_resp = client
        .post(&endpoint)
        .headers(headers)
        .json(&probe_body(kind, &model, true))
        .send()
        .await
        .map_err(|e| format!("流式请求发送失败: {e}"))?;
    if !stream_resp.status().is_success() {
        let status_code = stream_resp.status();
        let body = stream_resp.text().await.unwrap_or_default();
        steps.push(HealthCheckStep {
            name: "流式返回".into(),
            ok: false,
            detail: format!("{}: {}", status_code, body.chars().take(200).collect::<String>()),
        });
    } else {
        use futures_util::StreamExt;
        let mut stream = stream_resp.bytes_stream();
        let mut seen = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("读取流式响应失败: {e}"))?;
            seen += chunk.len();
            if seen > 0 {
                break;
            }
        }
        steps.push(HealthCheckStep {
            name: "流式返回".into(),
            ok: seen > 0,
            detail: if seen > 0 {
                format!("流式连接成功，首个数据块 {} 字节", seen)
            } else {
                "未收到流式数据".into()
            },
        });
    }

    let ok = steps.iter().all(|s| s.ok);
    Ok(HealthCheckReport {
        tool,
        base_url: base,
        model,
        ok,
        elapsed_ms: started.elapsed().as_millis(),
        steps,
    })
}

#[tauri::command]
fn usage_snapshot() -> Result<usage::UsageSnapshot, String> {
    usage::scan().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn health_check(tool: String) -> Result<HealthCheckReport, String> {
    health_probe(tool).await
}

// ----------------------------------------------------------- sessions management

/// Extract a title from a Claude Code project conversation JSONL.
/// Look for the first `type: "user"` message, read `message.content`.
fn extract_claude_conversation_title(path: &std::path::Path) -> String {
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未命名")
        .to_string();
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return fallback,
    };
    use std::io::{BufRead, BufReader};
    let reader = BufReader::new(file);
    for line in reader.lines().take(50).map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if msg_type == "user" {
            // message.content can be a string or an array of blocks
            if let Some(msg) = v.get("message") {
                if let Some(content) = msg.get("content") {
                    if let Some(text) = content.as_str() {
                        let result: String =
                            text.lines().next().unwrap_or("").chars().take(80).collect();
                        if !result.is_empty() {
                            return result;
                        }
                    }
                    if let Some(arr) = content.as_array() {
                        for block in arr {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                let result: String =
                                    text.lines().next().unwrap_or("").chars().take(80).collect();
                                if !result.is_empty() {
                                    return result;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fallback
}

/// Extract a title from a Codex session JSONL file.
/// The actual user prompt is in a line with:
///   `type: "event_msg"` + `payload.type: "user_message"` + `payload.message: "..."`
fn extract_codex_session_title(path: &std::path::Path) -> String {
    let fallback = {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if let Some(rest) = stem.strip_prefix("rollout-") {
            let date_part: String = rest.chars().take(16).collect();
            date_part
                .replacen('T', " ", 1)
                .chars()
                .enumerate()
                .map(|(i, c)| if i >= 11 && c == '-' { ':' } else { c })
                .collect()
        } else {
            stem.to_string()
        }
    };

    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return fallback,
    };
    use std::io::{BufRead, BufReader};
    let reader = BufReader::new(file);
    for line in reader.lines().take(30).map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        // The real user prompt is in event_msg with payload.type == "user_message"
        if line_type == "event_msg" {
            if let Some(payload) = v.get("payload") {
                let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if payload_type == "user_message" {
                    if let Some(msg) = payload.get("message").and_then(|m| m.as_str()) {
                        let result: String =
                            msg.lines().next().unwrap_or("").chars().take(80).collect();
                        if !result.is_empty() {
                            return result;
                        }
                    }
                }
            }
        }
    }
    fallback
}

#[derive(serde::Serialize, Clone)]
struct SessionEntry {
    id: String,
    tool: String,
    name: String,
    modified: i64,
    size_bytes: u64,
    path: String,
}

#[tauri::command]
fn list_sessions() -> Result<Vec<SessionEntry>, String> {
    let home = dirs::home_dir().ok_or("无法定位用户主目录")?;
    let mut out = Vec::new();

    // Claude Code: conversations in ~/.claude/projects/<path>/<session>.jsonl
    let projects_dir = home.join(".claude").join("projects");
    if projects_dir.exists() {
        if let Ok(project_dirs) = std::fs::read_dir(&projects_dir) {
            for project_entry in project_dirs.flatten() {
                let project_path = project_entry.path();
                if !project_path.is_dir() {
                    continue;
                }
                // The directory name is an encoded path like "D--dev-beeapi"
                let project_name = project_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .replace("--", "/")
                    .replace('-', "/");
                if let Ok(sessions) = std::fs::read_dir(&project_path) {
                    for session_entry in sessions.flatten() {
                        let path = session_entry.path();
                        if !path.is_file() {
                            continue;
                        }
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext != "jsonl" {
                            continue;
                        }
                        let meta = std::fs::metadata(&path).ok();
                        let modified = meta
                            .as_ref()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let size = meta.map(|m| m.len()).unwrap_or(0);
                        if size < 50 {
                            continue; // skip near-empty files
                        }
                        let title = extract_claude_conversation_title(&path);
                        let display_name = if title.len() > 3 {
                            title
                        } else {
                            format!("[{}]", project_name)
                        };
                        out.push(SessionEntry {
                            id: path.display().to_string(),
                            tool: "claude-code".into(),
                            name: display_name,
                            modified,
                            size_bytes: size,
                            path: path.display().to_string(),
                        });
                    }
                }
            }
        }
    }

    // Codex: ~/.codex/sessions/YYYY/MM/DD/*.jsonl (recursive)
    let codex_dir = codex_root(&home).join("sessions");
    if codex_dir.exists() {
        fn walk_codex(dir: &std::path::Path, out: &mut Vec<SessionEntry>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk_codex(&path, out);
                    } else if path.is_file() {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext != "jsonl" && ext != "json" {
                            continue;
                        }
                        let meta = std::fs::metadata(&path).ok();
                        let modified = meta
                            .as_ref()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let size = meta.map(|m| m.len()).unwrap_or(0);
                        let name = extract_codex_session_title(&path);
                        out.push(SessionEntry {
                            id: path.display().to_string(),
                            tool: "codex".into(),
                            name,
                            modified,
                            size_bytes: size,
                            path: path.display().to_string(),
                        });
                    }
                }
            }
        }
        walk_codex(&codex_dir, &mut out);
    }

    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(out)
}

#[tauri::command]
fn delete_session(path: String) -> Result<(), String> {
    let p = ensure_session_target(std::path::Path::new(&path))?;
    std::fs::remove_file(&p).map_err(|e| format!("delete failed: {e}"))?;
    Ok(())
}

/// Export a session JSONL file as Markdown with timestamps.
#[tauri::command]
fn export_session_markdown(path: String) -> Result<String, String> {
    use std::io::{BufRead, BufReader};

    let p = ensure_session_target(std::path::Path::new(&path))?;
    let file = std::fs::File::open(&p).map_err(|e| format!("open file failed: {e}"))?;
    let reader = BufReader::new(file);

    let mut md = String::new();
    md.push_str("# Session Export\n\n");

    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let timestamp = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
        let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match line_type {
            // Codex: user message
            "event_msg" => {
                if let Some(payload) = v.get("payload") {
                    let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if payload_type == "user_message" {
                        if let Some(msg) = payload.get("message").and_then(|m| m.as_str()) {
                            md.push_str(&format!(
                                "## User - {}\n\n{}\n\n---\n\n",
                                timestamp, msg
                            ));
                        }
                    } else if payload_type == "agent_reasoning" {
                        if let Some(text) = payload.get("text").and_then(|t| t.as_str()) {
                            md.push_str(&format!("> {}\n\n", text));
                        }
                    }
                }
            }
            // Codex: assistant response
            "response_item" => {
                if let Some(payload) = v.get("payload") {
                    let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let item_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if role == "assistant" && item_type == "message" {
                        if let Some(content) = payload.get("content").and_then(|c| c.as_array()) {
                            for block in content {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    md.push_str(&format!(
                                        "## Assistant - {}\n\n{}\n\n---\n\n",
                                        timestamp, text
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            // Claude Code: user message
            "user" => {
                if let Some(msg) = v.get("message") {
                    if let Some(content) = msg.get("content") {
                        let text = if let Some(s) = content.as_str() {
                            s.to_string()
                        } else if let Some(arr) = content.as_array() {
                            arr.iter()
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("\\n")
                        } else {
                            continue;
                        };
                        if !text.is_empty() {
                            md.push_str(&format!(
                                "## User - {}\n\n{}\n\n---\n\n",
                                timestamp, text
                            ));
                        }
                    }
                }
            }
            // Claude Code: assistant message
            "message" => {
                let role = v
                    .get("message")
                    .and_then(|m| m.get("role"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("");
                if role == "assistant" {
                    if let Some(content) = v.get("message").and_then(|m| m.get("content")) {
                        let text = if let Some(s) = content.as_str() {
                            s.to_string()
                        } else if let Some(arr) = content.as_array() {
                            arr.iter()
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("\\n")
                        } else {
                            continue;
                        };
                        if !text.is_empty() {
                            md.push_str(&format!(
                                "## Assistant - {}\n\n{}\n\n---\n\n",
                                timestamp, text
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if md.len() <= 20 {
        return Err("no conversation content could be extracted".into());
    }
    Ok(md)
}

// ----------------------------------------------------------- installs

#[derive(serde::Serialize)]
struct InstallStatus {
    tool: String,
    installed: bool,
    path: Option<String>,
}

#[tauri::command]
fn list_installed() -> Vec<InstallStatus> {
    const TOOLS: &[&str] = &[
        "claude-code",
        "codex",
        "gemini-cli",
        "opencode",
        "openclaw",
        "hermes",
    ];
    TOOLS
        .iter()
        .map(|t| {
            let located = detect::locate(t);
            InstallStatus {
                tool: t.to_string(),
                installed: located.is_some(),
                path: located.map(|p| p.display().to_string()),
            }
        })
        .collect()
}

// ----------------------------------------------------------- launch CLI in directory

#[tauri::command]
fn launch_cli_in_dir(tool: String, dir: String) -> Result<(), String> {
    use std::process::Command;

    let program = match tool.as_str() {
        "claude-code" => "claude",
        "codex" => "codex",
        "gemini-cli" => "gemini",
        "opencode" => "opencode",
        "openclaw" => "openclaw",
        "hermes" => "hermes",
        other => return Err(format!("未知工具: {other}")),
    };

    let dir_path = std::path::Path::new(&dir);
    if !dir_path.exists() || !dir_path.is_dir() {
        return Err(format!("目录不存在: {dir}"));
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, open a new cmd window with the CLI
        Command::new("cmd")
            .args(["/c", "start", "cmd", "/k", program])
            .current_dir(dir_path)
            .spawn()
            .map_err(|e| format!("启动 {program} 失败: {e}"))?;
    }

    #[cfg(target_os = "macos")]
    {
        // On macOS, open a new Terminal window
        let script = format!(
            "tell application \"Terminal\" to do script \"cd '{}' && {}\"",
            dir.replace('\'', "'\\''"),
            program
        );
        Command::new("osascript")
            .args(["-e", &script])
            .spawn()
            .map_err(|e| format!("启动 {program} 失败: {e}"))?;
    }

    #[cfg(target_os = "linux")]
    {
        // Try common terminal emulators
        let terminals = ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"];
        let mut launched = false;
        for term in &terminals {
            let result = if *term == "gnome-terminal" {
                Command::new(term)
                    .args(["--working-directory", &dir, "--", program])
                    .spawn()
            } else {
                Command::new(term)
                    .args(["-e", program])
                    .current_dir(dir_path)
                    .spawn()
            };
            if result.is_ok() {
                launched = true;
                break;
            }
        }
        if !launched {
            return Err(format!("未找到可用的终端模拟器来启动 {program}"));
        }
    }

    Ok(())
}

// ----------------------------------------------------------- window ctrls

#[tauri::command]
fn window_minimize(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|e| e.to_string())
}

#[tauri::command]
fn window_toggle_maximize(window: tauri::Window) -> Result<bool, String> {
    let maximized = window.is_maximized().map_err(|e| e.to_string())?;
    if maximized {
        window.unmaximize().map_err(|e| e.to_string())?;
    } else {
        window.maximize().map_err(|e| e.to_string())?;
    }
    Ok(!maximized)
}

#[tauri::command]
fn window_close(window: tauri::Window) -> Result<(), String> {
    // Hide to tray instead of closing — the frontend will ask the user first
    window.hide().map_err(|e| e.to_string())
}

#[tauri::command]
fn window_quit(app: tauri::AppHandle) -> Result<(), String> {
    let _ = persist_settings();
    app.exit(0);
    Ok(())
}

// ----------------------------------------------------------- system tray

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;

fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let p = proxy();

    let show_item = MenuItemBuilder::with_id("show", "显示主窗口").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let tooltip = build_tray_tooltip(&p);

    let mut tray_builder = TrayIconBuilder::with_id("main-tray")
        .tooltip(&tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(w) = tray.app_handle().get_webview_window("main") {
                    w.show().ok();
                    w.unminimize().ok();
                    w.set_focus().ok();
                }
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    w.show().ok();
                    w.unminimize().ok();
                    w.set_focus().ok();
                }
            }
            "quit" => {
                let _ = persist_settings();
                app.exit(0);
            }
            _ => {}
        });

    if let Some(icon) = app.default_window_icon() {
        tray_builder = tray_builder.icon(icon.clone());
    }

    let _tray = tray_builder.build(app)?;

    // Spawn a background task to update the tray tooltip every 5 seconds
    let app_handle = app.handle().clone();
    std::thread::Builder::new()
        .name("tray-updater".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let p = proxy();
            if let Some(tray) = app_handle.tray_by_id("main-tray") {
                let tip = build_tray_tooltip(&p);
                tray.set_tooltip(Some(&tip)).ok();
            }
            let _ = persist_settings();
        })
        .ok();

    Ok(())
}

fn build_tray_tooltip(p: &ProxyHandle) -> String {
    let key_count = p.pool.stats().iter().filter(|k| k.enabled).count();
    if p.pool.pool_enabled() {
        format!(
            "BeeAPI Switch\n密钥池: 开启\n可用密钥: {} 把\n上游: {}",
            key_count,
            p.upstream()
        )
    } else {
        let primary = get_primary_key().unwrap_or_else(|| "未设置".to_string());
        format!(
            "BeeAPI Switch\n直连模式\n主密钥: {}\n上游: {}",
            primary,
            p.upstream()
        )
    }
}

// ----------------------------------------------------------- codex plugin unlock via CDP

#[cfg(windows)]
fn find_standard_codex_path() -> Option<std::path::PathBuf> {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let path = std::path::Path::new(&local_app_data)
            .join("Programs")
            .join("Codex")
            .join("Codex.exe");
        if path.exists() {
            return Some(path);
        }
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let path = std::path::Path::new(&program_files)
            .join("Codex")
            .join("Codex.exe");
        if path.exists() {
            return Some(path);
        }
    }
    None
}

#[cfg(windows)]
const CLSID_APPLICATION_ACTIVATION_MANAGER: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x45BA127D,
    data2: 0x10A8,
    data3: 0x46EA,
    data4: [0x8A, 0xB7, 0x5D, 0xBE, 0x78, 0x59, 0x83, 0x57],
};

#[cfg(windows)]
const IID_IAPPLICATION_ACTIVATION_MANAGER: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x2E94148B,
    data2: 0x1E23,
    data3: 0x455E,
    data4: [0x84, 0x9F, 0x22, 0x46, 0x1D, 0x25, 0x11, 0x01],
};

#[cfg(windows)]
unsafe fn activate_uwp_app(aumid: &str, arguments: &str) -> Result<u32, String> {
    let aumid_str = aumid.to_string();
    let arguments_str = arguments.to_string();

    // 1. Try ShellExecuteW (Cleanest, bypassing COM local server registration issues)
    let wide_operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let wide_file: Vec<u16> = format!("shell:AppsFolder\\{}", aumid_str).encode_utf16().chain(Some(0)).collect();
    let wide_parameters: Vec<u16> = arguments_str.encode_utf16().chain(Some(0)).collect();

    let instance = windows_sys::Win32::UI::Shell::ShellExecuteW(
        std::ptr::null_mut(),
        wide_operation.as_ptr(),
        wide_file.as_ptr(),
        wide_parameters.as_ptr(),
        std::ptr::null(),
        1, // SW_SHOWNORMAL
    );

    if (instance as isize) > 32 {
        return Ok(0);
    }

    // 2. Try PowerShell Start-Process as a robust fallback
    let ps_command = format!(
        "Start-Process 'shell:AppsFolder\\{}' -ArgumentList '{}'",
        aumid_str, arguments_str
    );
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("powershell")
        .args(&["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps_command])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            return Ok(0);
        }
    }

    // 3. Fallback to the original COM ApplicationActivationManager logic
    std::thread::spawn(move || {
        use windows_sys::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
        };

        #[repr(C)]
        struct IApplicationActivationManagerVtbl {
            query_interface: unsafe extern "system" fn(
                this: *mut std::ffi::c_void,
                riid: *const windows_sys::core::GUID,
                ppv_object: *mut *mut std::ffi::c_void,
            ) -> i32,
            add_ref: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
            release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
            activate_application: unsafe extern "system" fn(
                this: *mut std::ffi::c_void,
                app_user_model_id: *const u16,
                arguments: *const u16,
                options: u32,
                process_id: *mut u32,
            ) -> i32,
        }

        #[repr(C)]
        struct IApplicationActivationManager {
            lp_vtbl: *const IApplicationActivationManagerVtbl,
        }

        let wide_aumid: Vec<u16> = aumid_str.encode_utf16().chain(Some(0)).collect();
        let wide_args: Vec<u16> = arguments_str.encode_utf16().chain(Some(0)).collect();

        let hr = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED as u32);
        let should_uninitialize = hr >= 0;

        let mut manager_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_APPLICATION_ACTIVATION_MANAGER,
            std::ptr::null_mut(),
            CLSCTX_LOCAL_SERVER,
            &IID_IAPPLICATION_ACTIVATION_MANAGER,
            &mut manager_ptr,
        );

        if hr < 0 {
            if should_uninitialize {
                CoUninitialize();
            }
            return Err(format!("创建 ApplicationActivationManager 实例失败: HRESULT 0x{hr:X}"));
        }

        let manager = manager_ptr as *mut IApplicationActivationManager;
        let mut process_id: u32 = 0;

        let hr = ((*(*manager).lp_vtbl).activate_application)(
            manager as *mut std::ffi::c_void,
            wide_aumid.as_ptr(),
            wide_args.as_ptr(),
            0,
            &mut process_id,
        );

        ((*(*manager).lp_vtbl).release)(manager as *mut std::ffi::c_void);

        if should_uninitialize {
            CoUninitialize();
        }

        if hr < 0 {
            return Err(format!("激活 UWP 应用失败: HRESULT 0x{hr:X}"));
        }

        Ok(process_id)
    })
    .join()
    .map_err(|e| format!("COM 线程发生恐慌: {:?}", e))?
}

#[cfg(windows)]
fn find_codex_aumid() -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let path = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\Repository\\Families";
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_READ) {
        for subkey_name in key.enum_keys().flatten() {
            if subkey_name.starts_with("OpenAI.Codex_") {
                return Some(format!("{}!App", subkey_name));
            }
        }
    }
    None
}

#[cfg(windows)]
fn launch_codex_with_debug_port() -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    // 1. Terminate any running instances of Codex.exe or OpenAI.Codex.exe
    let _ = std::process::Command::new("taskkill")
        .args(&["/F", "/IM", "Codex.exe"])
        .creation_flags(0x08000000)
        .output();
    let _ = std::process::Command::new("taskkill")
        .args(&["/F", "/IM", "OpenAI.Codex.exe"])
        .creation_flags(0x08000000)
        .output();

    std::thread::sleep(std::time::Duration::from_millis(500));

    let args = "--remote-debugging-port=9222 --remote-allow-origins=http://127.0.0.1:9222";

    // 2. Check if UWP version AUMID is registered and try to launch it
    let mut uwp_err = None;
    if let Some(aumid) = find_codex_aumid() {
        unsafe {
            match activate_uwp_app(&aumid, args) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    uwp_err = Some(e);
                }
            }
        }
    }

    // 3. Fallback to standard executable version
    if let Some(exe_path) = find_standard_codex_path() {
        std::process::Command::new(exe_path)
            .args(&["--remote-debugging-port=9222", "--remote-allow-origins=http://127.0.0.1:9222"])
            .spawn()
            .map_err(|e| format!("启动 Codex 可执行文件失败: {e}"))?;
        return Ok(());
    }

    if let Some(err) = uwp_err {
        Err(format!("UWP 启动失败 ({})，且未找到 Codex 标准安装版。", err))
    } else {
        Err("未在系统中检测到 Codex 安装路径。请确保已安装 Codex Desktop。".to_string())
    }
}

/// Ports the Codex Electron app might expose for CDP.
const CODEX_CDP_PORTS: &[u16] = &[9222, 9229, 9230, 9333];

/// Try to discover the running Codex CDP port.
#[tauri::command]
async fn codex_cdp_port() -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .build()
        .map_err(|e| format!("HTTP 客户端错误: {e}"))?;
    for &port in CODEX_CDP_PORTS {
        let url = format!("http://127.0.0.1:{port}/json");
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(port);
            }
        }
    }
    Err("未检测到 Codex 调试端口。请确保 Codex 已启动且启用了 remote-debugging-port".to_string())
}

/// The JavaScript that unlocks plugin entry and force-enables install buttons.
/// Ported from CodexPlusPlus renderer-inject.js.
fn plugin_unlock_script() -> String {
    r#"
(function() {
  // ---- helpers ----
  function reactFiberFrom(el) {
    const key = Object.keys(el).find(k => k.startsWith("__reactFiber"));
    return key ? el[key] : null;
  }
  function authContextValueFrom(el) {
    for (let fiber = reactFiberFrom(el); fiber; fiber = fiber.return) {
      for (const val of [fiber.memoizedProps?.value, fiber.pendingProps?.value]) {
        if (val && typeof val === "object" && typeof val.setAuthMethod === "function" && "authMethod" in val) {
          return val;
        }
      }
    }
    return null;
  }
  function spoofChatGPTAuth(el) {
    const auth = authContextValueFrom(el);
    if (!auth || auth.authMethod === "chatgpt") return false;
    auth.setAuthMethod("chatgpt");
    return true;
  }

  // ---- plugin entry unlock ----
  const navSelector = 'nav[role="navigation"] button.h-token-nav-row.w-full';
  const svgSelector = 'svg path[d^="M7.94562 14.0277"]';
  function pluginButton() {
    // 1. Specific navigation bar + icon match
    let btn = document.querySelector(navSelector + " " + svgSelector)?.closest("button");
    if (btn) return btn;

    // 2. Specific navigation bar + text content match
    btn = Array.from(document.querySelectorAll(navSelector))
      .find(b => /^(插件|Plugins)(\s+-\s+.*)?$/i.test((b.textContent || "").trim()));
    if (btn) return btn;

    // 3. Global icon match fallback
    btn = document.querySelector('button ' + svgSelector)?.closest("button")
      || document.querySelector(svgSelector)?.closest("button");
    if (btn) return btn;

    // 4. Global text content match fallback
    btn = Array.from(document.querySelectorAll('button'))
      .find(b => /^(插件|Plugins)(\s+-\s+.*)?$/i.test((b.textContent || "").trim()));
    return btn || null;
  }
  function unlockEntry() {
    const btn = pluginButton();
    if (!btn) return false;
    spoofChatGPTAuth(btn);
    btn.disabled = false;
    btn.removeAttribute("disabled");
    btn.style.display = "";
    btn.querySelectorAll("*").forEach(n => { n.style.display = ""; });
    // label
    const labelNode = Array.from(btn.querySelectorAll("span, div")).reverse()
      .flatMap(n => Array.from(n.childNodes))
      .find(n => n.nodeType === 3 && /^(插件|Plugins)( - 已解锁| - Unlocked)?$/i.test((n.nodeValue || "").trim()));
    if (labelNode) {
      labelNode.nodeValue = /^Plugins/i.test((labelNode.nodeValue || "").trim()) ? "Plugins - Unlocked" : "插件 - 已解锁";
    }
    const rpKey = Object.keys(btn).find(k => k.startsWith("__reactProps"));
    if (rpKey) btn[rpKey].disabled = false;
    if (btn.dataset.codexPluginEnabled !== "true") {
      btn.dataset.codexPluginEnabled = "true";

      const keepUnlocked = () => {
        spoofChatGPTAuth(btn);
        btn.disabled = false;
        btn.removeAttribute("disabled");
        const k = Object.keys(btn).find(k => k.startsWith("__reactProps"));
        if (k) btn[k].disabled = false;
      };

      // Add extensive event listeners to instantly unlock the button when interacted with,
      // avoiding React's own re-renders resetting the disabled state.
      ["pointerdown", "mousedown", "mouseup", "click", "focus", "mouseenter", "mouseover"].forEach((eventName) => {
        btn.addEventListener(eventName, keepUnlocked, true);
      });
      keepUnlocked();
    }
    return true;
  }

  // ---- force install unlock ----
  const disabledSel = 'button:disabled, button[aria-disabled="true"], [role="button"][aria-disabled="true"], button[data-disabled], [role="button"][data-disabled], button.cursor-not-allowed, [role="button"].cursor-not-allowed';
  function patchReactProps(el) {
    Object.keys(el).filter(k => k.startsWith("__reactProps")).forEach(k => {
      const p = el[k];
      if (!p || typeof p !== "object") return;
      p.disabled = false;
      p["aria-disabled"] = false;
      p["data-disabled"] = undefined;
    });
  }
  function clearDisabled(el) {
    if (!(el instanceof HTMLElement)) return;
    if ("disabled" in el) el.disabled = false;
    el.removeAttribute("disabled");
    el.removeAttribute("aria-disabled");
    el.removeAttribute("data-disabled");
    el.removeAttribute("inert");
    el.classList.remove("disabled", "opacity-50", "cursor-not-allowed", "pointer-events-none");
    el.style.pointerEvents = "auto";
    el.style.opacity = "";
    el.style.cursor = "pointer";
    el.tabIndex = 0;
    patchReactProps(el);
  }
  function unlockInstallButtons() {
    const candidates = Array.from(new Set(
      Array.from(document.querySelectorAll(disabledSel))
        .map(n => n.closest?.("button, [role='button']") || n)
    ));
    candidates.forEach(btn => {
      const text = (btn.textContent || "").trim();
      if (!/^(安装|Install)\s*/.test(text) && text !== "强制安装") return;
      [btn, ...btn.querySelectorAll?.("button, [role='button'], [disabled], [aria-disabled], [data-disabled], .cursor-not-allowed, .pointer-events-none") || []].forEach(clearDisabled);
      // relabel
      const walker = document.createTreeWalker(btn, NodeFilter.SHOW_TEXT);
      let textNode = null;
      while (!textNode && walker.nextNode()) {
        const n = walker.currentNode;
        if (/^(安装|Install)\s*/i.test((n.nodeValue || "").trim())) textNode = n;
      }
      if (textNode) textNode.nodeValue = "强制安装";
    });
  }

  // ---- add style ----
  if (!document.getElementById("beeapi-plugin-unlock-style")) {
    const s = document.createElement("style");
    s.id = "beeapi-plugin-unlock-style";
    s.textContent = ".codex-force-install-unlocked { border-color: #ef4444 !important; background: #fee2e2 !important; color: #991b1b !important; opacity: 1 !important; }";
    document.documentElement.appendChild(s);
  }

  // ---- run ----
  const entryOk = unlockEntry();
  unlockInstallButtons();

  // ---- keep alive ----
  clearInterval(window.__beeapiPluginUnlockTimer);
  window.__beeapiPluginUnlockTimer = setInterval(() => {
    unlockEntry();
    unlockInstallButtons();
  }, 1200);

  return entryOk ? "plugin_entry_unlocked" : "install_buttons_only";
})();
"#
    .to_string()
}

/// Connect to the running Codex via CDP and inject the plugin unlock script.
#[tauri::command]
async fn unlock_codex_plugins() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .map_err(|e| format!("HTTP 客户端错误: {e}"))?;

    let mut active_port = None;
    let mut page_targets: Vec<serde_json::Value> = Vec::new();

    // 1. Initial quick check if Codex is already running and ready (has at least one page target)
    for &port in CODEX_CDP_PORTS {
        let url = format!("http://127.0.0.1:{port}/json");
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(targets) = resp.json::<Vec<serde_json::Value>>().await {
                    let pages: Vec<serde_json::Value> = targets
                        .into_iter()
                        .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
                        .filter(|t| t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()).is_some())
                        .collect();
                    if !pages.is_empty() {
                        active_port = Some(port);
                        page_targets = pages;
                        break;
                    }
                }
            }
        }
    }

    // 2. If no active port/page target is found, attempt to launch Codex with debug port on Windows
    if active_port.is_none() {
        #[cfg(windows)]
        {
            launch_codex_with_debug_port()?;

            // Poll for up to 60 times (30 seconds total) to wait for Codex to start and initialize its window
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;

                let mut found_port = None;
                for &port in CODEX_CDP_PORTS {
                    let url = format!("http://127.0.0.1:{port}/json");
                    if let Ok(resp) = client.get(&url).send().await {
                        if resp.status().is_success() {
                            if let Ok(targets) = resp.json::<Vec<serde_json::Value>>().await {
                                let pages: Vec<serde_json::Value> = targets
                                    .into_iter()
                                    .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
                                    .filter(|t| t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()).is_some())
                                    .collect();
                                if !pages.is_empty() {
                                    found_port = Some(port);
                                    page_targets = pages;
                                    break;
                                }
                            }
                        }
                    }
                }

                if let Some(port) = found_port {
                    active_port = Some(port);
                    break;
                }
            }
        }
    }

    let _port = active_port.ok_or_else(|| {
        "未检测到 Codex 调试端口。请确保已启动 Codex 客户端且调试端口已开启。".to_string()
    })?;

    if page_targets.is_empty() {
        return Err("未找到可注入的 Codex 页面目标。请确保 Codex 窗口已打开。".to_string());
    }

    // Prioritize main index.html page target and avoid overlay/sub-windows (containing initialRoute)
    let codex_target = page_targets
        .iter()
        .find(|t| {
            let url = t.get("url").and_then(|v| v.as_str()).unwrap_or("");
            url.ends_with("/index.html") || (url.contains("index.html") && !url.contains("initialRoute="))
        })
        .or_else(|| {
            page_targets.iter().find(|t| {
                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let url = t.get("url").and_then(|v| v.as_str()).unwrap_or("");
                title.contains("Codex") || title.contains("codex") || url.contains("codex")
            })
        })
        .unwrap_or(&page_targets[0]);

    let ws_url = codex_target
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .ok_or("CDP 目标缺少 WebSocket 调试 URL")?;

    // 4. Use WebSocket to send Runtime.evaluate
    use tokio_tungstenite::connect_async;
    use futures_util::{SinkExt, StreamExt};

    let (mut ws, _) = connect_async(ws_url)
        .await
        .map_err(|e| format!("连接 CDP WebSocket 失败: {e}"))?;

    let script = plugin_unlock_script();
    let evaluate_msg = serde_json::json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": {
            "expression": script,
            "awaitPromise": false,
            "allowUnsafeEvalBlockedByCSP": true,
            "returnByValue": true
        }
    });

    let evaluate_text = serde_json::to_string(&evaluate_msg)
        .map_err(|e| format!("序列化 CDP 命令失败: {e}"))?;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        evaluate_text.into(),
    ))
    .await
    .map_err(|e| format!("发送 CDP 命令失败: {e}"))?;

    // 5. Wait for response
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        if val.get("id").and_then(|v| v.as_i64()) == Some(1) {
                            return Ok(val);
                        }
                    }
                }
                Err(e) => return Err(format!("CDP WebSocket 错误: {e}")),
                _ => {}
            }
        }
        Err("CDP WebSocket 连接意外关闭".to_string())
    });

    let result = timeout
        .await
        .map_err(|_| "等待 CDP 响应超时".to_string())??;

    // Check for errors
    if let Some(err) = result.get("error") {
        let msg = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("未知错误");
        return Err(format!("CDP 执行错误: {msg}"));
    }

    let return_value = result
        .pointer("/result/result/value")
        .and_then(|v| v.as_str())
        .unwrap_or("ok");

    let _ = ws.close(None).await;

    match return_value {
        "plugin_entry_unlocked" => Ok("插件入口已解锁，安装按钮已启用".to_string()),
        "install_buttons_only" => Ok("安装按钮已解锁（未找到插件入口按钮，可能页面未加载完成）".to_string()),
        _ => Ok("插件解锁脚本已注入".to_string()),
    }
}

// ----------------------------------------------------------- entry point

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let stored = pool_store::load().unwrap_or_default();
    let upstream = match stored.upstream.as_deref().map(str::trim) {
        Some("https://bate.beeapi.ai") | Some("https://bate.beeapi.ai/") => {
            proxy::DEFAULT_UPSTREAM.to_string()
        }
        Some(s) if !s.is_empty() => s.to_string(),
        _ => proxy::DEFAULT_UPSTREAM.to_string(),
    };
    let options = StartOptions {
        upstream,
        token: stored
            .local_token
            .clone()
            .unwrap_or_else(proxy::random_token),
        lan: stored.lan.unwrap_or(false),
        pool_enabled: stored.pool_enabled.unwrap_or(false),
        port: proxy::DEFAULT_PORT,
    };
    let handle = match proxy::start(options) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("启动本地代理失败: {e:#}");
            return;
        }
    };
    if !stored.keys.is_empty() {
        handle.pool.set_keys(stored.keys.clone());
    }
    if let Some(threshold) = stored.fail_threshold {
        handle.pool.set_fail_threshold(threshold);
    }
    handle.pool.set_persisted_totals(
        stored.total_input_tokens.unwrap_or(0),
        stored.total_output_tokens.unwrap_or(0),
        stored.total_cache_read_tokens.unwrap_or(0),
        stored.total_cache_write_tokens.unwrap_or(0),
    );
    handle
        .pool
        .set_preferred_key_id(stored.primary_key_id.clone());
    set_primary_key(stored.primary_key_id.clone());
    let arc = Arc::new(handle);
    PROXY.set(arc).ok();
    let _ = persist_settings();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let urls = argv
                .into_iter()
                .filter(|arg| DEEP_LINK_SCHEMES.iter().any(|scheme| arg.starts_with(&format!("{scheme}:"))))
                .collect::<Vec<_>>();
            if !urls.is_empty() {
                handle_deep_link_urls(app, urls);
            } else if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            // On Windows, remove native decorations for custom titlebar.
            #[cfg(target_os = "windows")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window.set_decorations(false).ok();
                }
            }

            // System tray: always create, tooltip shows token usage when pool is on.
            setup_tray(app)?;

            let handle = app.handle().clone();
            let startup_urls = collect_startup_deep_link_urls();
            if !startup_urls.is_empty() {
                handle_deep_link_urls(&handle, startup_urls);
            }

            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;

                let callback_handle = handle.clone();
                handle.deep_link().on_open_url(move |event| {
                    let urls = event.urls().iter().map(|u| u.to_string()).collect::<Vec<_>>();
                    handle_deep_link_urls(&callback_handle, urls);
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Intercept the native close event (Alt+F4, titlebar close, etc.)
            // and let the frontend show the same "minimize or quit" dialog.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit("beeapi-close-requested", ());
            }
        })
        .invoke_handler(tauri::generate_handler![
            apply_config,
            clear_config,
            list_status,
            list_config_backups,
            restore_config_backup,
            rollback_config_backup,
            delete_config_backup,
            clear_config_backups,
            fetch_models,
            download_image_to,
            sync_codex_history,
            rollback_latest_codex_history_backup,
            list_installed,
            proxy_health,
            proxy_info,
            set_lan,
            set_pool_enabled,
            set_primary_key_id,
            regenerate_token,
            set_fail_threshold,
            register_deep_link_service,
            pool_stats,
            token_usage,
            reset_token_usage,
            verify_key,
            set_pool_keys,
            load_pool,
            import_from_deep_link,
            take_pending_import_notice,
            request_log,
            clear_request_log,
            export_request_log_csv,
            query_usage,
            usage_snapshot,
            health_check,
            export_usage_csv,
            unlock_codex_plugins,
            list_sessions,
            delete_session,
            export_session_markdown,
            launch_cli_in_dir,
            codex_cdp_port,
            window_minimize,
            window_toggle_maximize,
            window_close,
            window_quit,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| eprintln!("error while running tauri application: {e}"));
}
