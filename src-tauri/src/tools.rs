//! Per-tool writer implementations. Each writer knows how to map a
//! `(api_key, model)` pair into one or more on-disk config files for a
//! specific CLI, and how to detect/remove an existing BeeAPI configuration.
//!
//! Everything goes through the single BeeAPI host:
//!   - OpenAI-compatible API:   https://beeapi.ai/v1
//!   - Anthropic-compatible API: https://beeapi.ai  (path: /v1/messages)
//!   - Gemini-compatible API:   https://beeapi.ai
//!
//! Users set a single base URL via the UI; this module just writes it to
//! whatever shape each CLI expects.

use std::path::PathBuf;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};

/// Default BeeAPI root.
pub const DEFAULT_BASE: &str = "https://beeapi.ai/";

/// Tag we embed in files we manage so we can detect and remove them later.
pub const BEEAPI_MARKER: &str = "beeapi.ai";

/// Outcome of writing files for a tool.
pub struct WriteOutcome {
    /// Files that were (over)written.
    pub files: Vec<PathBuf>,
}

/// Status about whether a tool currently points at BeeAPI.
pub struct StatusOutcome {
    pub configured: bool,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub key: Option<String>,
    pub wire_api: Option<String>,
}

/// Resolve the base URL in its three flavours from a user-supplied root.
/// Accepts either `https://beeapi.ai` or `https://beeapi.ai/v1`.
fn normalize_base(base: &str) -> (String, String) {
    let trimmed = base.trim().trim_end_matches('/').to_string();
    let (root, openai) = if trimmed.ends_with("/v1") {
        let root = trimmed.trim_end_matches("/v1").to_string();
        (root, trimmed)
    } else {
        (trimmed.clone(), format!("{}/v1", trimmed))
    };
    (root, openai)
}

/// Codex home directory. Respects `CODEX_HOME` when present.
pub fn codex_root(home: &PathBuf) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

// ---------------------------------------------------------------------------
// Claude Code  (Anthropic-compatible)
// File: ~/.claude/settings.json
// ---------------------------------------------------------------------------

pub fn claude_code_paths(home: &PathBuf) -> Vec<PathBuf> {
    vec![home.join(".claude").join("settings.json")]
}

pub fn claude_code_apply(
    home: &PathBuf,
    api_key: &str,
    model: &str,
    base: &str,
) -> Result<WriteOutcome> {
    let (root, _) = normalize_base(base);
    let settings = home.join(".claude").join("settings.json");

    let mut value = read_json_or_empty(&settings)?;
    let obj = ensure_object(&mut value)?;

    let env = obj
        .entry("env".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let env_obj = env
        .as_object_mut()
        .ok_or_else(|| anyhow!("settings.json 的 env 字段不是对象"))?;

    env_obj.insert("ANTHROPIC_BASE_URL".into(), Value::String(root));
    env_obj.insert("ANTHROPIC_AUTH_TOKEN".into(), Value::String(api_key.into()));
    env_obj.insert("ANTHROPIC_MODEL".into(), Value::String(model.into()));
    env_obj.insert(
        "ANTHROPIC_SMALL_FAST_MODEL".into(),
        Value::String(model.into()),
    );
    env_obj.insert("BEEAPI_MANAGED".into(), Value::String(BEEAPI_MARKER.into()));

    write_json_pretty(&settings, &value)?;
    Ok(WriteOutcome {
        files: vec![settings],
    })
}

pub fn claude_code_status(home: &PathBuf) -> Result<StatusOutcome> {
    let settings = home.join(".claude").join("settings.json");
    if !settings.exists() {
        return Ok(StatusOutcome::empty());
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings)?)?;
    let env = v.get("env").and_then(|x| x.as_object());
    let base = env
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|x| x.as_str())
        .map(String::from);
    let model = env
        .and_then(|e| e.get("ANTHROPIC_MODEL"))
        .and_then(|x| x.as_str())
        .map(String::from);
    let key = env
        .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|x| x.as_str())
        .map(String::from);
    let configured = env
        .and_then(|e| e.get("BEEAPI_MANAGED"))
        .and_then(|x| x.as_str())
        .map(|s| s == BEEAPI_MARKER)
        .unwrap_or_else(|| {
            base.as_deref()
                .map(|b| b.contains("beeapi.ai") || b.contains("127.0.0.1"))
                .unwrap_or(false)
        });
    Ok(StatusOutcome {
        configured,
        base_url: base,
        model,
        key,
        ..StatusOutcome::empty()
    })
}

pub fn claude_code_clear(home: &PathBuf) -> Result<()> {
    let settings = home.join(".claude").join("settings.json");
    if !settings.exists() {
        return Ok(());
    }
    let mut root = read_json_or_empty(&settings)?;
    if let Some(obj) = root.as_object_mut() {
        if let Some(env) = obj.get_mut("env").and_then(|x| x.as_object_mut()) {
            for k in [
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_MODEL",
                "ANTHROPIC_SMALL_FAST_MODEL",
                "BEEAPI_MANAGED",
            ] {
                env.remove(k);
            }
        }
    }
    write_json_pretty(&settings, &root)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Codex  (OpenAI-compatible)
// Files: ~/.codex/config.toml , ~/.codex/auth.json
// ---------------------------------------------------------------------------

pub fn codex_paths(home: &PathBuf) -> Vec<PathBuf> {
    let root = codex_root(home);
    vec![root.join("config.toml"), root.join("auth.json")]
}

pub fn codex_apply(home: &PathBuf, api_key: &str, model: &str, base: &str) -> Result<WriteOutcome> {
    use toml_edit::{value, DocumentMut, Item, Table};

    let (_, openai) = normalize_base(base);
    let root = codex_root(home);
    let cfg_path = root.join("config.toml");
    let auth_path = root.join("auth.json");

    let existing = if cfg_path.exists() {
        std::fs::read_to_string(&cfg_path).unwrap_or_default()
    } else {
        String::new()
    };
    let mut doc: DocumentMut = existing.parse().unwrap_or_else(|_| DocumentMut::new());

    doc["model"] = value(model);
    doc["model_provider"] = value("beeapi");
    doc["preferred_auth_method"] = value("apikey");
    doc["disable_response_storage"] = value(true);
    doc["beeapi_managed"] = value(BEEAPI_MARKER);

    if !matches!(doc.get("model_providers"), Some(Item::Table(_))) {
        doc["model_providers"] = Item::Table(Table::new());
    }
    let providers = doc["model_providers"]
        .as_table_mut()
        .ok_or_else(|| anyhow!("config.toml 的 model_providers 不是表"))?;
    providers.set_implicit(true);

    let mut bee = Table::new();
    bee["name"] = value("BeeAPI");
    bee["base_url"] = value(openai.as_str());
    bee["wire_api"] = value("responses");
    providers.insert("beeapi", Item::Table(bee));

    write_text(&cfg_path, &doc.to_string())?;

    let auth = json!({
        "OPENAI_API_KEY": api_key,
        "beeapi_managed": BEEAPI_MARKER
    });
    write_json_pretty(&auth_path, &auth)?;

    Ok(WriteOutcome {
        files: vec![cfg_path, auth_path],
    })
}

pub fn codex_status(home: &PathBuf) -> Result<StatusOutcome> {
    let root = codex_root(home);
    let cfg = root.join("config.toml");
    if !cfg.exists() {
        return Ok(StatusOutcome::empty());
    }
    let text = std::fs::read_to_string(&cfg).unwrap_or_default();
    let configured =
        text.contains("beeapi_managed") || text.contains("beeapi.ai") || text.contains("127.0.0.1");
    let doc: toml_edit::DocumentMut = text.parse().unwrap_or_default();
    let model = doc.get("model").and_then(|v| v.as_str()).map(String::from);
    let provider = doc.get("model_provider").and_then(|v| v.as_str());
    let provider_table = doc
        .get("model_providers")
        .and_then(|mp| mp.as_table())
        .and_then(|providers| {
            provider
                .and_then(|id| providers.get(id))
                .or_else(|| providers.get("beeapi"))
        })
        .and_then(|b| b.as_table());
    let base_url = provider_table
        .and_then(|t| t.get("base_url"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let wire_api = provider_table
        .and_then(|t| t.get("wire_api"))
        .and_then(|v| v.as_str())
        .map(String::from);
    // Read key from auth.json
    let auth_path = root.join("auth.json");
    let key = if auth_path.exists() {
        std::fs::read_to_string(&auth_path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| {
                v.get("OPENAI_API_KEY")
                    .and_then(|k| k.as_str())
                    .map(String::from)
            })
    } else {
        None
    };
    Ok(StatusOutcome {
        configured,
        base_url,
        model,
        key,
        wire_api,
    })
}

pub fn codex_clear(home: &PathBuf) -> Result<()> {
    let root = codex_root(home);
    let cfg_path = root.join("config.toml");
    if cfg_path.exists() {
        let text = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap_or_default();
        for k in [
            "model",
            "model_provider",
            "preferred_auth_method",
            "disable_response_storage",
            "beeapi_managed",
        ] {
            doc.remove(k);
        }
        if let Some(providers) = doc
            .get_mut("model_providers")
            .and_then(|i| i.as_table_mut())
        {
            providers.remove("beeapi");
        }
        write_text(&cfg_path, &doc.to_string())?;
    }
    let auth = root.join("auth.json");
    if auth.exists() {
        if let Ok(text) = std::fs::read_to_string(&auth) {
            if text.contains(BEEAPI_MARKER) {
                let _ = std::fs::remove_file(&auth);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Gemini CLI  (uses the OpenAI-compatible endpoint via env vars)
// Files: ~/.gemini/settings.json , ~/.gemini/.env
// ---------------------------------------------------------------------------

pub fn gemini_paths(home: &PathBuf) -> Vec<PathBuf> {
    vec![
        home.join(".gemini").join("settings.json"),
        home.join(".gemini").join(".env"),
    ]
}

pub fn gemini_apply(
    home: &PathBuf,
    api_key: &str,
    model: &str,
    base: &str,
) -> Result<WriteOutcome> {
    let (root, openai) = normalize_base(base);
    let settings = home.join(".gemini").join("settings.json");
    let env_path = home.join(".gemini").join(".env");

    let mut value = read_json_or_empty(&settings)?;
    let obj = ensure_object(&mut value)?;
    obj.insert(
        "selectedAuthType".into(),
        Value::String("gemini-api-key".into()),
    );
    obj.insert("model".into(), Value::String(model.into()));
    obj.insert("beeapiManaged".into(), Value::String(BEEAPI_MARKER.into()));
    write_json_pretty(&settings, &value)?;

    let env_contents = format!(
        "# Managed by BeeAPI Switch - {marker}\n\
         GEMINI_API_KEY={key}\n\
         GOOGLE_API_KEY={key}\n\
         GOOGLE_GEMINI_BASE_URL={root}\n\
         OPENAI_API_KEY={key}\n\
         OPENAI_BASE_URL={openai}\n",
        marker = BEEAPI_MARKER,
        key = api_key,
        root = root,
        openai = openai,
    );
    write_text(&env_path, &env_contents)?;

    Ok(WriteOutcome {
        files: vec![settings, env_path],
    })
}

pub fn gemini_status(home: &PathBuf) -> Result<StatusOutcome> {
    let env_path = home.join(".gemini").join(".env");
    if !env_path.exists() {
        return Ok(StatusOutcome::empty());
    }
    let text = std::fs::read_to_string(&env_path).unwrap_or_default();
    let configured =
        text.contains(BEEAPI_MARKER) || text.contains("beeapi.ai") || text.contains("127.0.0.1");
    let base_url = text
        .lines()
        .find_map(|l| l.strip_prefix("GOOGLE_GEMINI_BASE_URL="))
        .map(|s| s.trim().to_string());
    let key = text
        .lines()
        .find_map(|l| l.strip_prefix("GEMINI_API_KEY="))
        .map(|s| s.trim().to_string());
    let settings = home.join(".gemini").join("settings.json");
    let model = if settings.exists() {
        serde_json::from_str::<Value>(&std::fs::read_to_string(&settings)?)
            .ok()
            .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
    } else {
        None
    };
    Ok(StatusOutcome {
        configured,
        base_url,
        model,
        key,
        ..StatusOutcome::empty()
    })
}

pub fn gemini_clear(home: &PathBuf) -> Result<()> {
    let env_path = home.join(".gemini").join(".env");
    if env_path.exists() {
        if let Ok(t) = std::fs::read_to_string(&env_path) {
            if t.contains(BEEAPI_MARKER) {
                let _ = std::fs::remove_file(&env_path);
            }
        }
    }
    let settings = home.join(".gemini").join("settings.json");
    if settings.exists() {
        let mut root = read_json_or_empty(&settings)?;
        if let Some(obj) = root.as_object_mut() {
            obj.remove("beeapiManaged");
        }
        write_json_pretty(&settings, &root)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// OpenCode  (OpenAI-compatible)
// File: ~/.config/opencode/opencode.json
// ---------------------------------------------------------------------------

pub fn opencode_paths(home: &PathBuf) -> Vec<PathBuf> {
    vec![home.join(".config").join("opencode").join("opencode.json")]
}

pub fn opencode_apply(
    home: &PathBuf,
    api_key: &str,
    model: &str,
    base: &str,
) -> Result<WriteOutcome> {
    let (_, openai) = normalize_base(base);
    let path = home.join(".config").join("opencode").join("opencode.json");

    let mut value = read_json_or_empty(&path)?;
    let obj = ensure_object(&mut value)?;
    obj.remove("beeapiManaged");

    obj.entry("$schema".to_string())
        .or_insert_with(|| Value::String("https://opencode.ai/config.json".into()));

    let providers = obj
        .entry("provider".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| anyhow!("opencode.json 的 provider 不是对象"))?;

    for provider in providers.values_mut() {
        if let Some(provider_obj) = provider.as_object_mut() {
            provider_obj.remove("beeapiManaged");
        }
    }

    let mut models_map = Map::new();
    models_map.insert(model.to_string(), Value::Object(Map::new()));

    let beeapi = json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "BeeAPI",
        "options": {
            "baseURL": openai,
            "apiKey": api_key
        },
        "models": Value::Object(models_map)
    });
    providers.insert("beeapi".into(), beeapi);

    obj.insert("model".into(), Value::String(format!("beeapi/{model}")));

    write_json_pretty(&path, &value)?;
    Ok(WriteOutcome { files: vec![path] })
}

pub fn opencode_status(home: &PathBuf) -> Result<StatusOutcome> {
    let path = home.join(".config").join("opencode").join("opencode.json");
    if !path.exists() {
        return Ok(StatusOutcome::empty());
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let beeapi = v.get("provider").and_then(|p| p.get("beeapi"));
    let configured = beeapi.is_some();
    let base_url = beeapi
        .and_then(|b| b.get("options"))
        .and_then(|o| o.get("baseURL"))
        .and_then(|u| u.as_str())
        .map(String::from);
    let key = beeapi
        .and_then(|b| b.get("options"))
        .and_then(|o| o.get("apiKey"))
        .and_then(|k| k.as_str())
        .map(String::from);
    let model = v.get("model").and_then(|m| m.as_str()).map(String::from);
    Ok(StatusOutcome {
        configured,
        base_url,
        model,
        key,
        ..StatusOutcome::empty()
    })
}

pub fn opencode_clear(home: &PathBuf) -> Result<()> {
    let path = home.join(".config").join("opencode").join("opencode.json");
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_or_empty(&path)?;
    if let Some(obj) = root.as_object_mut() {
        if let Some(providers) = obj.get_mut("provider").and_then(|p| p.as_object_mut()) {
            providers.remove("beeapi");
        }
        if obj
            .get("model")
            .and_then(|m| m.as_str())
            .map(|m| m.starts_with("beeapi/"))
            .unwrap_or(false)
        {
            obj.remove("model");
        }
    }
    write_json_pretty(&path, &root)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// OpenClaw  (Anthropic-compatible)
// File: ~/.openclaw/config.json
// ---------------------------------------------------------------------------

pub fn openclaw_paths(home: &PathBuf) -> Vec<PathBuf> {
    vec![home.join(".openclaw").join("config.json")]
}

pub fn openclaw_apply(
    home: &PathBuf,
    api_key: &str,
    model: &str,
    base: &str,
) -> Result<WriteOutcome> {
    let (root, _) = normalize_base(base);
    let path = home.join(".openclaw").join("config.json");

    let mut value = read_json_or_empty(&path)?;
    let obj = ensure_object(&mut value)?;

    let env = obj
        .entry("env".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| anyhow!("openclaw config.json 的 env 字段不是对象"))?;

    env.insert("ANTHROPIC_BASE_URL".into(), Value::String(root));
    env.insert("ANTHROPIC_AUTH_TOKEN".into(), Value::String(api_key.into()));
    env.insert("ANTHROPIC_MODEL".into(), Value::String(model.into()));
    env.insert(
        "ANTHROPIC_SMALL_FAST_MODEL".into(),
        Value::String(model.into()),
    );
    env.insert("BEEAPI_MANAGED".into(), Value::String(BEEAPI_MARKER.into()));

    write_json_pretty(&path, &value)?;
    Ok(WriteOutcome { files: vec![path] })
}

pub fn openclaw_status(home: &PathBuf) -> Result<StatusOutcome> {
    let path = home.join(".openclaw").join("config.json");
    if !path.exists() {
        return Ok(StatusOutcome::empty());
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let env = v.get("env").and_then(|e| e.as_object());
    let managed = env
        .and_then(|e| e.get("BEEAPI_MANAGED"))
        .and_then(|m| m.as_str())
        .map(|m| m == BEEAPI_MARKER)
        .unwrap_or(false);
    let base = env
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|m| m.as_str())
        .map(String::from);
    let configured = managed
        || base
            .as_deref()
            .map(|b| b.contains("beeapi.ai") || b.contains("127.0.0.1"))
            .unwrap_or(false);
    let model = env
        .and_then(|e| e.get("ANTHROPIC_MODEL"))
        .and_then(|m| m.as_str())
        .map(String::from);
    let key = env
        .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|k| k.as_str())
        .map(String::from);
    Ok(StatusOutcome {
        configured,
        base_url: base,
        model,
        key,
        ..StatusOutcome::empty()
    })
}

pub fn openclaw_clear(home: &PathBuf) -> Result<()> {
    let path = home.join(".openclaw").join("config.json");
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_json_or_empty(&path)?;
    if let Some(obj) = root.as_object_mut() {
        if let Some(env) = obj.get_mut("env").and_then(|e| e.as_object_mut()) {
            for k in [
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_MODEL",
                "ANTHROPIC_SMALL_FAST_MODEL",
                "BEEAPI_MANAGED",
            ] {
                env.remove(k);
            }
        }
    }
    write_json_pretty(&path, &root)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Hermes  (OpenAI-compatible)
// File: ~/.hermes/config.yaml
// ---------------------------------------------------------------------------

pub fn hermes_paths(home: &PathBuf) -> Vec<PathBuf> {
    vec![home.join(".hermes").join("config.yaml")]
}

pub fn hermes_apply(
    home: &PathBuf,
    api_key: &str,
    model: &str,
    base: &str,
) -> Result<WriteOutcome> {
    let (_, openai) = normalize_base(base);
    let path = home.join(".hermes").join("config.yaml");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let cleaned = remove_hermes_managed_yaml(&existing, true);
    let text = render_hermes_yaml(&cleaned, api_key, model, &openai);
    write_text(&path, &text)?;
    Ok(WriteOutcome { files: vec![path] })
}

pub fn hermes_status(home: &PathBuf) -> Result<StatusOutcome> {
    let path = home.join(".hermes").join("config.yaml");
    if !path.exists() {
        return Ok(StatusOutcome::empty());
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let base_url = find_yaml_value_in_block(&text, "model", "base_url")
        .or_else(|| find_yaml_value(&text, "base_url"));
    let key = find_yaml_value_in_block(&text, "model", "api_key")
        .or_else(|| find_yaml_value(&text, "api_key"));
    let model = find_yaml_value_in_block(&text, "model", "model")
        .or_else(|| find_yaml_value_in_block(&text, "model", "default"))
        .or_else(|| find_yaml_value(&text, "model"))
        .map(|m| m.strip_prefix("beeapi/").unwrap_or(&m).to_string());
    let configured = text.contains(BEEAPI_MARKER)
        || base_url
            .as_deref()
            .map(|b| b.contains("beeapi.ai") || b.contains("127.0.0.1"))
            .unwrap_or(false);
    Ok(StatusOutcome {
        configured,
        base_url,
        model,
        key,
        ..StatusOutcome::empty()
    })
}

pub fn hermes_clear(home: &PathBuf) -> Result<()> {
    let path = home.join(".hermes").join("config.yaml");
    if !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let cleaned = remove_hermes_managed_yaml(&text, true);
    if cleaned.trim().is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        write_text(&path, cleaned.trim_end())?;
    }
    Ok(())
}

fn render_hermes_yaml(existing: &str, api_key: &str, model: &str, openai_base: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("# Managed by BeeAPI Switch - {BEEAPI_MARKER}"));
    lines.push("model:".to_string());
    // Hermes' current config schema reads the active provider from model.provider.
    // A custom OpenAI-compatible endpoint is selected with provider=custom.
    lines.push("  provider: custom".to_string());
    lines.push(format!("  default: {}", yaml_quote(model)));
    lines.push(format!("  model: {}", yaml_quote(model)));
    lines.push(format!("  base_url: {}", yaml_quote(openai_base)));
    lines.push(format!("  api_key: {}", yaml_quote(api_key)));
    lines.push("  api_mode: chat_completions".to_string());
    lines.push(format!("  beeapi_managed: {}", yaml_quote(BEEAPI_MARKER)));

    let trimmed_existing = existing.trim();
    if !trimmed_existing.is_empty() {
        lines.push(String::new());
        lines.extend(trimmed_existing.lines().map(|line| line.to_string()));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn remove_hermes_managed_yaml(text: &str, remove_top_level_selection: bool) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        let indent = yaml_indent(line);

        if trimmed == format!("# Managed by BeeAPI Switch - {BEEAPI_MARKER}") {
            i += 1;
            continue;
        }

        if remove_top_level_selection && indent == 0 && is_top_level_yaml_key(line, "model") {
            i = skip_yaml_block(&lines, i);
            continue;
        }

        if remove_top_level_selection
            && indent == 0
            && (is_top_level_yaml_key(line, "provider")
                || is_top_level_yaml_key(line, "beeapi_managed"))
        {
            i += 1;
            continue;
        }

        // Drop the legacy BeeAPI provider block generated by earlier builds.
        if indent > 0 && trimmed.starts_with("beeapi:") {
            i = skip_yaml_block(&lines, i);
            continue;
        }

        out.push(line);
        i += 1;
    }

    out.join("\n")
}

fn skip_yaml_block(lines: &[&str], start: usize) -> usize {
    let block_indent = yaml_indent(lines[start]);
    let mut i = start + 1;
    while i < lines.len() {
        let next = lines[i];
        let next_trimmed = next.trim();
        if next_trimmed.is_empty() {
            i += 1;
            continue;
        }
        if yaml_indent(next) <= block_indent {
            break;
        }
        i += 1;
    }
    i
}

fn is_top_level_yaml_key(line: &str, key: &str) -> bool {
    yaml_indent(line) == 0 && line.trim_start().starts_with(&format!("{key}:"))
}

fn yaml_indent(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 2 } else { 1 })
        .sum()
}

fn yaml_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn find_yaml_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("{key}:");
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&needle) {
            let raw = rest.trim();
            if raw.is_empty() {
                continue;
            }
            return Some(unquote_yaml_scalar(raw));
        }
    }
    None
}

fn find_yaml_value_in_block(text: &str, block_key: &str, value_key: &str) -> Option<String> {
    let needle = format!("{value_key}:");
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if is_top_level_yaml_key(line, block_key) {
            let block_indent = yaml_indent(line);
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                let next_trimmed = next.trim();
                if !next_trimmed.is_empty() && yaml_indent(next) <= block_indent {
                    break;
                }
                if let Some(rest) = next_trimmed.strip_prefix(&needle) {
                    let raw = rest.trim();
                    if !raw.is_empty() {
                        return Some(unquote_yaml_scalar(raw));
                    }
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    None
}

fn unquote_yaml_scalar(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('"') && value.ends_with('"') {
        serde_json::from_str::<String>(value).unwrap_or_else(|_| {
            value
                .trim_start_matches('"')
                .trim_end_matches('"')
                .to_string()
        })
    } else if value.starts_with('\'') && value.ends_with('\'') {
        value
            .trim_start_matches('\'')
            .trim_end_matches('\'')
            .replace("''", "'")
    } else {
        value.to_string()
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

impl StatusOutcome {
    fn empty() -> Self {
        Self {
            configured: false,
            base_url: None,
            model: None,
            key: None,
            wire_api: None,
        }
    }
}

pub fn read_json_or_empty(path: &std::path::Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("读取 {} 失败", path.display()))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(trimmed)
        .with_context(|| format!("解析 {} 时 JSON 格式错误", path.display()))
}

pub fn ensure_object(v: &mut Value) -> Result<&mut Map<String, Value>> {
    if !v.is_object() {
        *v = Value::Object(Map::new());
    }
    v.as_object_mut()
        .ok_or_else(|| anyhow!("JSON value is not an object after normalization"))
}

pub fn write_json_pretty(path: &std::path::Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value)?;
    atomic_write(path, text.as_bytes())
}

pub fn write_text(path: &std::path::Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    atomic_write(path, text.as_bytes())
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("target path has no parent: {}", path.display()))?;
    let tmp = parent.join(format!(
        ".{}.beeapi-tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config")
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("failed to write temporary file {}", tmp.display()))?;

    let backup = parent.join(format!(
        ".{}.beeapi-bak",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config")
    ));
    if path.exists() {
        if backup.exists() {
            let _ = std::fs::remove_file(&backup);
        }
        std::fs::rename(path, &backup)
            .with_context(|| format!("failed to backup existing {}", path.display()))?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, path);
        }
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()));
    }
    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
    Ok(())
}
