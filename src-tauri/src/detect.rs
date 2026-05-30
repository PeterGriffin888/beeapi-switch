//! Detect whether the target CLI is installed on the user's machine.
//! Checks PATH, and also known install locations (e.g. VS Code extensions
//! for Codex, npm global installs for Claude Code, etc.).

use std::path::PathBuf;

use crate::tools::codex_root;

/// Candidate executable base names for each tool.
fn candidates(tool: &str) -> &'static [&'static str] {
    match tool {
        "claude-code" => &["claude", "claude-code"],
        "codex" => &["codex"],
        "gemini-cli" => &["gemini"],
        "opencode" => &["opencode", "oc"],
        "openclaw" => &["openclaw", "claw"],
        "hermes" => &["hermes"],
        _ => &[],
    }
}

fn path_extensions() -> Vec<String> {
    #[cfg(windows)]
    {
        let raw = std::env::var("PATHEXT")
            .unwrap_or_else(|_| String::from(".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WS;.MSC;.PS1"));
        let mut v: Vec<String> = raw
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().to_ascii_lowercase())
            .collect();
        v.push(String::new());
        v
    }
    #[cfg(not(windows))]
    {
        vec![String::new()]
    }
}

/// Walk PATH looking for any of the candidate binary names.
fn locate_on_path(tool: &str) -> Option<PathBuf> {
    let cands = candidates(tool);
    if cands.is_empty() {
        return None;
    }
    let path_var = std::env::var_os("PATH")?;
    let exts = path_extensions();

    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for base in cands {
            for ext in &exts {
                let file_name = if ext.is_empty() {
                    (*base).to_string()
                } else if ext.starts_with('.') {
                    format!("{base}{ext}")
                } else {
                    format!("{base}.{ext}")
                };
                let candidate = dir.join(&file_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Check well-known install locations beyond PATH.
fn locate_known_paths(tool: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    match tool {
        "codex" => {
            // VS Code Codex extension (OpenAI Codex plugin)
            let vscode_ext = home.join(".vscode").join("extensions");
            if vscode_ext.exists() {
                if let Ok(entries) = std::fs::read_dir(&vscode_ext) {
                    for entry in entries.flatten() {
                        let name = entry.file_name();
                        let name_str = name.to_string_lossy();
                        if name_str.starts_with("openai.codex")
                            || name_str.starts_with("openai.openai-codex")
                        {
                            return Some(entry.path());
                        }
                    }
                }
            }
            // Also check ~/.codex directory existence as a sign of installation
            let codex_dir = codex_root(&home);
            if codex_dir.exists() {
                return Some(codex_dir);
            }
            None
        }
        "claude-code" => {
            // Check if ~/.claude exists (sign of Claude Code being used)
            let claude_dir = home.join(".claude");
            if claude_dir.join("settings.json").exists() {
                return Some(claude_dir);
            }
            // npm global: check common npm prefix locations
            #[cfg(windows)]
            {
                let appdata = std::env::var("APPDATA").ok()?;
                let npm_global = PathBuf::from(appdata).join("npm").join("claude.cmd");
                if npm_global.exists() {
                    return Some(npm_global);
                }
            }
            None
        }
        "gemini-cli" => {
            // Only detect via PATH — ~/.gemini existing doesn't mean the CLI is installed
            None
        }
        "opencode" => {
            // Only detect via PATH
            None
        }
        "openclaw" => {
            // Only detect via PATH
            None
        }
        "hermes" => {
            // Only detect via PATH
            None
        }
        _ => None,
    }
}

/// Primary entry point: try PATH first, then known locations.
pub fn locate(tool: &str) -> Option<PathBuf> {
    locate_on_path(tool).or_else(|| locate_known_paths(tool))
}

#[allow(dead_code)]
pub fn is_installed(tool: &str) -> bool {
    locate(tool).is_some()
}
