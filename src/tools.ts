export type ToolId =
  | "claude-code"
  | "codex"
  | "gemini-cli"
  | "opencode"
  | "openclaw"
  | "hermes";

export interface ToolMeta {
  id: ToolId;
  name: string;
  /** File paths that will be written/modified. */
  paths: string[];
}

export const TOOLS: ToolMeta[] = [
  {
    id: "claude-code",
    name: "Claude Code",
    paths: ["~/.claude/settings.json"],
  },
  {
    id: "codex",
    name: "Codex",
    paths: ["~/.codex/config.toml", "~/.codex/auth.json"],
  },
  {
    id: "gemini-cli",
    name: "Gemini CLI",
    paths: ["~/.gemini/settings.json", "~/.gemini/.env"],
  },
  {
    id: "opencode",
    name: "OpenCode",
    paths: ["~/.config/opencode/opencode.json"],
  },
  {
    id: "openclaw",
    name: "OpenClaw",
    paths: ["~/.openclaw/config.json"],
  },
  {
    id: "hermes",
    name: "Hermes",
    paths: ["~/.hermes/config.yaml"],
  },
];
