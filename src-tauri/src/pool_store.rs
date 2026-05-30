//! Persistence for key pool, upstream, local-token, LAN toggle and the
//! "pool enabled" switch. Stored in `~/.beeapi-switch/settings.json`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::proxy::ApiKey;

#[derive(Default, Serialize, Deserialize)]
pub struct StoredSettings {
    #[serde(default)]
    pub keys: Vec<ApiKey>,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default)]
    pub local_token: Option<String>,
    #[serde(default)]
    pub lan: Option<bool>,
    #[serde(default)]
    pub pool_enabled: Option<bool>,
    /// ID of the key to use for direct (non-pool) mode.
    #[serde(default)]
    pub primary_key_id: Option<String>,
    /// How many consecutive failures before cooling down a key.
    #[serde(default)]
    pub fail_threshold: Option<u32>,
    /// Accumulated total token usage across all sessions.
    #[serde(default)]
    pub total_input_tokens: Option<u64>,
    #[serde(default)]
    pub total_output_tokens: Option<u64>,
    #[serde(default)]
    pub total_cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub total_cache_write_tokens: Option<u64>,
}

fn store_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot locate home directory"))?;
    Ok(home.join(".beeapi-switch").join("settings.json"))
}

pub fn load() -> Result<StoredSettings> {
    let path = store_path()?;
    if !path.exists() {
        return Ok(StoredSettings::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(StoredSettings::default());
    }
    serde_json::from_str(&text).with_context(|| format!("failed to parse JSON in {}", path.display()))
}

pub fn save(settings: &StoredSettings) -> Result<()> {
    let path = store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(settings)?;
    atomic_write(&path, text.as_bytes())
}

fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("target path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;

    let unique = format!(
        "{}.{}.beeapi-tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("settings"),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let tmp = parent.join(unique);

    std::fs::write(&tmp, bytes)
        .with_context(|| format!("failed to write temporary file {}", tmp.display()))?;

    let backup = parent.join(format!(
        "{}.{}.beeapi-bak",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("settings"),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    if path.exists() {
        std::fs::rename(path, &backup)
            .with_context(|| format!("failed to backup existing {}", path.display()))?;
    }

    match std::fs::rename(&tmp, path) {
        Ok(_) => {
            if backup.exists() {
                let _ = std::fs::remove_file(&backup);
            }
            Ok(())
        }
        Err(e) => {
            if backup.exists() {
                let _ = std::fs::rename(&backup, path);
            }
            let _ = std::fs::remove_file(&tmp);
            Err(anyhow::anyhow!(
                "failed to rename {} to {}: {e}",
                tmp.display(),
                path.display()
            ))
        }
    }
}
