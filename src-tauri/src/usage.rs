use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

use crate::tools::codex_root;

#[derive(Clone, Serialize)]
pub struct UsageRecord {
    pub ts: i64,
    pub tool: String,
    pub session: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub path: String,
}

#[derive(Serialize)]
pub struct UsageSnapshot {
    pub files_scanned: usize,
    pub records: Vec<UsageRecord>,
}

#[derive(Default, Clone, Copy)]
struct Tokens {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

pub fn scan() -> Result<UsageSnapshot> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法定位用户主目录"))?;
    let mut records = Vec::new();
    let mut files_scanned = 0usize;

    let claude_dir = home.join(".claude").join("projects");
    if claude_dir.exists() {
        for path in walk_files(&claude_dir)? {
            if !is_jsonl_or_json(&path) {
                continue;
            }
            files_scanned += 1;
            records.extend(scan_claude_file(&path)?);
        }
    }

    let codex_dir = codex_root(&home).join("sessions");
    if codex_dir.exists() {
        for path in walk_files(&codex_dir)? {
            if !is_jsonl_or_json(&path) {
                continue;
            }
            files_scanned += 1;
            records.extend(scan_codex_file(&path)?);
        }
    }

    records.sort_by(|a, b| b.ts.cmp(&a.ts));
    Ok(UsageSnapshot {
        files_scanned,
        records,
    })
}

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_dir(dir, &mut out)?;
    Ok(out)
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn is_jsonl_or_json(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("jsonl") | Some("json")
    )
}

fn scan_claude_file(path: &Path) -> Result<Vec<UsageRecord>> {
    let file = File::open(path).with_context(|| format!("打开 {} 失败", path.display()))?;
    let reader = BufReader::new(file);
    let session = extract_claude_title(path);
    let mut out = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let message = match value.get("message") {
            Some(message) => message,
            None => continue,
        };
        let usage = match extract_usage_value(message) {
            Some(usage) => usage,
            None => continue,
        };
        let ts = match extract_timestamp(&value) {
            Some(ts) => ts,
            None => continue,
        };
        let model = extract_model(message).unwrap_or_else(|| "claude-code".to_string());
        let tokens = extract_tokens(usage);
        out.push(UsageRecord {
            ts,
            tool: "claude-code".to_string(),
            session: session.clone(),
            model,
            input_tokens: tokens.input,
            output_tokens: tokens.output,
            cache_read_tokens: tokens.cache_read,
            cache_write_tokens: tokens.cache_write,
            path: path.display().to_string(),
        });
    }

    Ok(out)
}

fn scan_codex_file(path: &Path) -> Result<Vec<UsageRecord>> {
    let file = File::open(path).with_context(|| format!("打开 {} 失败", path.display()))?;
    let reader = BufReader::new(file);
    let session = extract_codex_title(path);
    let mut out = Vec::new();
    let mut current_model: Option<String> = None;

    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if let Some(model) = extract_model(&value) {
            current_model = Some(model);
        }

        if value.get("type").and_then(|v| v.as_str()) == Some("event_msg") {
            let payload = match value.get("payload") {
                Some(payload) => payload,
                None => continue,
            };
            if let Some(model) = extract_model(payload) {
                current_model = Some(model);
            }
            if payload.get("type").and_then(|v| v.as_str()) != Some("token_count") {
                continue;
            }
            let usage = match extract_codex_token_count_usage(payload) {
                Some(usage) => usage,
                None => continue,
            };
            let ts = match extract_timestamp(&value) {
                Some(ts) => ts,
                None => continue,
            };
            let tokens = extract_tokens(usage);
            out.push(UsageRecord {
                ts,
                tool: "codex".to_string(),
                session: session.clone(),
                model: current_model.clone().unwrap_or_else(|| "codex".to_string()),
                input_tokens: tokens.input,
                output_tokens: tokens.output,
                cache_read_tokens: tokens.cache_read,
                cache_write_tokens: tokens.cache_write,
                path: path.display().to_string(),
            });
            continue;
        }

        if value.get("type").and_then(|v| v.as_str()) != Some("response_item") {
            continue;
        }
        let payload = match value.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        if payload.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let usage = match extract_usage_value(payload) {
            Some(usage) => usage,
            None => continue,
        };
        let ts = match extract_timestamp(&value) {
            Some(ts) => ts,
            None => continue,
        };
        let model = extract_model(payload)
            .or_else(|| current_model.clone())
            .unwrap_or_else(|| "codex".to_string());
        let tokens = extract_tokens(usage);
        out.push(UsageRecord {
            ts,
            tool: "codex".to_string(),
            session: session.clone(),
            model,
            input_tokens: tokens.input,
            output_tokens: tokens.output,
            cache_read_tokens: tokens.cache_read,
            cache_write_tokens: tokens.cache_write,
            path: path.display().to_string(),
        });
    }

    Ok(out)
}

fn extract_codex_token_count_usage(payload: &Value) -> Option<&Value> {
    payload
        .get("info")
        .and_then(|v| v.get("last_token_usage"))
        .filter(|v| v.is_object())
        .or_else(|| {
            payload
                .get("info")
                .and_then(|v| v.get("total_token_usage"))
                .filter(|v| v.is_object())
        })
        .or_else(|| extract_usage_value(payload))
}

fn extract_timestamp(value: &Value) -> Option<i64> {
    value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|dt| dt.timestamp())
}

fn extract_model(value: &Value) -> Option<String> {
    let candidates = [
        value.get("model"),
        value.get("message").and_then(|v| v.get("model")),
        value.get("payload").and_then(|v| v.get("model")),
        value
            .get("payload")
            .and_then(|v| v.get("message"))
            .and_then(|v| v.get("model")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Some(model) = candidate.as_str() {
            let trimmed = model.trim();
            if !trimmed.is_empty() && trimmed != "<synthetic>" {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_usage_value(value: &Value) -> Option<&Value> {
    value
        .get("usage")
        .filter(|v| v.is_object())
        .or_else(|| {
            value
                .get("message")
                .and_then(|v| v.get("usage"))
                .filter(|v| v.is_object())
        })
        .or_else(|| {
            value
                .get("payload")
                .and_then(|v| v.get("usage"))
                .filter(|v| v.is_object())
        })
        .or_else(|| {
            value
                .get("payload")
                .and_then(|v| v.get("message"))
                .and_then(|v| v.get("usage"))
                .filter(|v| v.is_object())
        })
}

fn extract_tokens(value: &Value) -> Tokens {
    let mut tokens = Tokens::default();
    tokens.input = number(value, &["input_tokens", "prompt_tokens"]);
    tokens.output = number(value, &["output_tokens", "completion_tokens"])
        + number(value, &["reasoning_output_tokens"]);
    tokens.cache_read = number(
        value,
        &[
            "cache_read_input_tokens",
            "cached_input_tokens",
            "cached_tokens",
            "prompt_cache_hit_tokens",
        ],
    );
    tokens.cache_write = number(
        value,
        &[
            "cache_creation_input_tokens",
            "prompt_cache_miss_tokens",
            "cache_write_tokens",
        ],
    );
    if tokens.cache_read == 0 {
        if let Some(details) = value.get("input_tokens_details") {
            tokens.cache_read = number(details, &["cached_tokens"]);
        }
    }
    if tokens.cache_write == 0 {
        if let Some(details) = value.get("prompt_tokens_details") {
            tokens.cache_write = number(details, &["cached_tokens"]);
        }
    }
    tokens
}

fn number(value: &Value, keys: &[&str]) -> u64 {
    for key in keys {
        if let Some(num) = value.get(*key).and_then(|v| v.as_u64()) {
            return num;
        }
    }
    0
}

fn extract_claude_title(path: &Path) -> String {
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未命名")
        .to_string();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return fallback,
    };
    let reader = BufReader::new(file);
    for line in reader.lines().take(80).map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("type").and_then(|v| v.as_str()) == Some("user") {
            if let Some(message) = value.get("message") {
                if let Some(content) = message.get("content") {
                    if let Some(text) = content.as_str() {
                        let first = text.lines().next().unwrap_or("").trim();
                        if !first.is_empty() {
                            return first.chars().take(80).collect();
                        }
                    }
                    if let Some(items) = content.as_array() {
                        for item in items {
                            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                let first = text.lines().next().unwrap_or("").trim();
                                if !first.is_empty() {
                                    return first.chars().take(80).collect();
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

fn extract_codex_title(path: &Path) -> String {
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未命名")
        .to_string();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return fallback,
    };
    let reader = BufReader::new(file);
    for line in reader.lines().take(80).map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("type").and_then(|v| v.as_str()) == Some("event_msg") {
            if let Some(payload) = value.get("payload") {
                if payload.get("type").and_then(|v| v.as_str()) == Some("user_message") {
                    if let Some(message) = payload.get("message").and_then(|v| v.as_str()) {
                        let first = message.lines().next().unwrap_or("").trim();
                        if !first.is_empty() {
                            return first.chars().take(80).collect();
                        }
                    }
                }
            }
        }
        if value.get("type").and_then(|v| v.as_str()) == Some("user") {
            if let Some(message) = value
                .get("message")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_str())
            {
                let first = message.lines().next().unwrap_or("").trim();
                if !first.is_empty() {
                    return first.chars().take(80).collect();
                }
            }
        }
    }
    fallback
}
