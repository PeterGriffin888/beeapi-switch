import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ToolIcon } from "./icons";
import { ToolId } from "./tools";
import { t } from "./i18n";
import ConfirmDialog from "./ConfirmDialog";

interface SessionEntry {
  id: string;
  tool: string;
  name: string;
  modified: number;
  size_bytes: number;
  path: string;
}

function fmtDate(ts: number): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function toolLabel(tool: string): string {
  switch (tool) {
    case "claude-code":
      return "Claude Code";
    case "codex":
      return "Codex";
    case "gemini-cli":
      return "Gemini CLI";
    default:
      return tool;
  }
}

type Filter = "all" | "claude-code" | "codex";

export default function SessionsView() {
  const [sessions, setSessions] = useState<SessionEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [days, setDays] = useState("all");
  const [notice, setNotice] = useState<string | null>(null);

  // Confirm dialog state
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [confirmTitle, setConfirmTitle] = useState("");
  const [confirmMessage, setConfirmMessage] = useState("");
  const [confirmAction, setConfirmAction] = useState<(() => void) | null>(null);

  useEffect(() => {
    refresh();
  }, []);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const list = await invoke<SessionEntry[]>("list_sessions");
      setSessions(list);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  function showConfirm(title: string, message: string, action: () => void) {
    setConfirmTitle(title);
    setConfirmMessage(message);
    setConfirmAction(() => action);
    setConfirmOpen(true);
  }

  function onConfirmDone() {
    setConfirmOpen(false);
    confirmAction?.();
  }

  function onConfirmCancel() {
    setConfirmOpen(false);
    setConfirmAction(null);
  }

  function onDelete(entry: SessionEntry) {
    showConfirm(
      t("dialog.deleteSession"),
      t("sessions.deleteConfirm", { name: entry.name }),
      async () => {
        try {
          await invoke("delete_session", { path: entry.path });
          setSessions((prev) => prev.filter((s) => s.id !== entry.id));
        } catch (e) {
          setError(String(e));
        }
      },
    );
  }

  async function onExport(entry: SessionEntry) {
    try {
      const md = await invoke<string>("export_session_markdown", { path: entry.path });
      const { save } = await import("@tauri-apps/plugin-dialog");
      const filePath = await save({
        defaultPath: `${entry.name.slice(0, 50).replace(/[<>:"/\\|?*]/g, "_")}.md`,
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (!filePath) return;
      const { writeTextFile } = await import("@tauri-apps/plugin-fs");
      await writeTextFile(filePath, md);
      setNotice(t("sessions.exportSuccess", { file: filePath.split(/[/\\]/).pop() || filePath }));
      setTimeout(() => setNotice(null), 3000);
    } catch (e) {
      setError(String(e));
    }
  }

  function onDeleteFiltered() {
    const filtered = visible();
    showConfirm(
      t("dialog.deleteSession"),
      t(days === "all" ? "sessions.deleteAllConfirm" : "sessions.deleteOldConfirm", { count: filtered.length, days }),
      async () => {
        for (const s of filtered) {
          try {
            await invoke("delete_session", { path: s.path });
          } catch {
            // continue
          }
        }
        await refresh();
      },
    );
  }

  function visible(): SessionEntry[] {
    const cutoff = days === "all" ? null : Date.now() / 1000 - Number(days) * 86400;
    const q = query.trim().toLowerCase();
    return sessions.filter((s) => {
      if (filter !== "all" && s.tool !== filter) return false;
      if (cutoff !== null && s.modified > cutoff) return false;
      if (q && !`${s.name} ${s.path} ${s.tool}`.toLowerCase().includes(q)) return false;
      return true;
    });
  }

  const filtered = visible();
  const claudeCount = sessions.filter((s) => s.tool === "claude-code").length;
  const codexCount = sessions.filter((s) => s.tool === "codex").length;

  return (
    <div className="panel view-enter" style={{ flex: 1 }}>
      <header className="panel-header">
        <div className="big-mark svg-mark">
          <svg
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M4 6 H20 M4 12 H20 M4 18 H14" />
          </svg>
        </div>
        <div style={{ flex: 1 }}>
          <h2>
            {t("sessions.title")}
            <span className="pill">{sessions.length}</span>
          </h2>
          <div className="sub">
            {t("sessions.desc")}
          </div>
        </div>
      </header>

      <div className="panel-divider" />

      <div
        style={{
          display: "flex",
          gap: 8,
          alignItems: "center",
          marginBottom: 14,
        }}
      >
        <button
          className={`btn small ${filter === "all" ? "primary" : ""}`}
          onClick={() => setFilter("all")}
        >
          {t("sessions.all")} ({sessions.length})
        </button>
        <button
          className={`btn small ${filter === "claude-code" ? "primary" : ""}`}
          onClick={() => setFilter("claude-code")}
        >
          Claude Code ({claudeCount})
        </button>
        <button
          className={`btn small ${filter === "codex" ? "primary" : ""}`}
          onClick={() => setFilter("codex")}
        >
          Codex ({codexCount})
        </button>
        <input
          className="input search-input"
          placeholder={t("sessions.search")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <select className="select small-select" value={days} onChange={(e) => setDays(e.target.value)}>
          <option value="all">{t("sessions.keepAll")}</option>
          <option value="7">{t("sessions.olderThan7")}</option>
          <option value="30">{t("sessions.olderThan30")}</option>
          <option value="90">{t("sessions.olderThan90")}</option>
        </select>
        <div style={{ flex: 1 }} />
        <button className="btn small" onClick={refresh} disabled={loading}>
          {t("sessions.refresh")}
        </button>
        <button
          className="btn small danger"
          onClick={onDeleteFiltered}
          disabled={filtered.length === 0}
        >
          {t("sessions.deleteFiltered")} ({filtered.length})
        </button>
      </div>

      {notice && (
        <div className="alert ok toast-in" style={{ position: "fixed", bottom: 20, right: 20, zIndex: 100 }}>
          <div style={{ flex: 1 }}>{notice}</div>
        </div>
      )}

      {error && (
        <div className="alert err toast-in">
          <div style={{ flex: 1 }}>
            <pre>{error}</pre>
          </div>
        </div>
      )}

      {loading ? (
        <div className="empty">
          <span className="spinner" style={{ width: 18, height: 18 }} />
          <div>{t("sessions.loading")}</div>
        </div>
      ) : filtered.length === 0 ? (
        <div className="empty">
          <div className="empty-icon">
            <svg
              width="28"
              height="28"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.4"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M4 6 H20 M4 12 H20 M4 18 H14" />
            </svg>
          </div>
          <div>{query || days !== "all" ? t("sessions.emptyFiltered") : t("sessions.empty")}</div>
        </div>
      ) : (
        <table className="log-table">
          <thead>
            <tr>
              <th style={{ width: 36 }}></th>
              <th>{t("sessions.session")}</th>
              <th style={{ width: 80 }}>{t("sessions.tool")}</th>
              <th style={{ width: 150 }}>{t("sessions.modified")}</th>
              <th style={{ width: 80 }}>{t("sessions.size")}</th>
              <th style={{ width: 110, textAlign: "right" }}>{t("sessions.action")}</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((s) => (
              <tr key={s.id} className="log-row">
                <td>
                  <ToolIcon id={s.tool as ToolId} size={16} />
                </td>
                <td
                  title={s.name}
                  style={{
                    maxWidth: 320,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {s.name}
                </td>
                <td>{toolLabel(s.tool)}</td>
                <td className="mono">{fmtDate(s.modified)}</td>
                <td className="mono">{fmtSize(s.size_bytes)}</td>
                <td style={{ textAlign: "right" }}>
                  <div style={{ display: "flex", gap: 4, justifyContent: "flex-end" }}>
                    <button
                      className="btn small"
                      onClick={() => onExport(s)}
                      title={t("sessions.exportHint")}
                    >
                      {t("sessions.export")}
                    </button>
                    <button
                      className="btn small danger"
                      onClick={() => onDelete(s)}
                      style={{ marginLeft: 0 }}
                    >
                      {t("keys.delete")}
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <ConfirmDialog
        open={confirmOpen}
        title={confirmTitle}
        message={confirmMessage}
        danger
        confirmLabel={t("keys.delete")}
        onConfirm={onConfirmDone}
        onCancel={onConfirmCancel}
      />
    </div>
  );
}
