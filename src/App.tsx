import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TOOLS, ToolId, ToolMeta } from "./tools";
import Titlebar, { TabId } from "./Titlebar";
import KeysView from "./KeysView";
import UsageView from "./UsageView";
import SessionsView from "./SessionsView";
import AboutView from "./AboutView";
import PlaygroundView from "./PlaygroundView";
import StatusBar from "./StatusBar";
import ConfirmDialog from "./ConfirmDialog";
import { ToolIcon } from "./icons";
import { humanizeError } from "./errors";
import { t } from "./i18n";
import { checkForAppUpdate } from "./updater";

interface ApplyResult {
  tool: string;
  files: string[];
  backup_dir: string | null;
}

interface ConfigStatus {
  tool: string;
  configured: boolean;
  base_url: string | null;
  model: string | null;
  key_hint: string | null;
}

interface BackupEntry {
  tool: string;
  name: string;
  path: string;
  modified: number;
  files: string[];
  base_url: string | null;
  model: string | null;
  key_hint: string | null;
}

interface ModelInfo {
  id: string;
  owned_by: string | null;
}

interface InstallStatus {
  tool: string;
  installed: boolean;
  path: string | null;
}

interface UsageInfo {
  is_valid: boolean;
  remaining: number | null;
  unit: string;
  invalid_message?: string;
  total?: number;
  used?: number;
  key_name?: string;
  key_prefix?: string;
  plan_name?: string;
  expires_at?: string;
}

interface HealthCheckStep {
  name: string;
  ok: boolean;
  detail: string;
}

interface HealthCheckReport {
  tool: string;
  base_url: string;
  model: string;
  ok: boolean;
  elapsed_ms: number;
  steps: HealthCheckStep[];
}

interface SyncHistoryReport {
  files_updated: number;
  db_updated: number;
  provider: string;
  backup_dir: string | null;
}

interface RollbackHistoryReport {
  backup_dir: string;
  files_restored: number;
  db_restored: boolean;
}

interface ImportNotice {
  id: number;
  ok: boolean;
  msg: string;
}

interface ImportPayload {
  keys_added: number;
  duplicates: number;
  pool_enabled: boolean | null;
  primary_key_id: string | null;
}

interface PendingImportNotice {
  ok: boolean;
  payload?: ImportPayload;
  error?: string;
}

type Theme = "light" | "dark";

function loadTheme(): Theme {
  const stored = localStorage.getItem("beeapi-theme") as Theme | null;
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function importSuccessMessage(p: ImportPayload) {
  return p.keys_added > 0
    ? t("keys.importSuccess", {
        added: p.keys_added,
        duplicates: p.duplicates,
      })
    : t("keys.importNoNew", { duplicates: p.duplicates });
}

export default function App() {
  const [tab, setTab] = useState<TabId>("tools");
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const [importNotice, setImportNotice] = useState<ImportNotice | null>(null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("beeapi-theme", theme);
  }, [theme]);

  useEffect(() => {
    void checkForAppUpdate();

    let unlistenImported: (() => void) | undefined;
    let unlistenImportError: (() => void) | undefined;
    listen<ImportPayload>("beeapi-imported", (event) => {
      const p = event.payload;
      setImportNotice({
        id: Date.now(),
        ok: true,
        msg: importSuccessMessage(p),
      });
      setTab("keys");
    }).then((fn) => {
      unlistenImported = fn;
    });
    listen<string>("beeapi-import-error", (event) => {
      setImportNotice({
        id: Date.now(),
        ok: false,
        msg: t("keys.importFailed", { error: event.payload }),
      });
      setTab("keys");
    }).then((fn) => {
      unlistenImportError = fn;
    });
    invoke<PendingImportNotice | null>("take_pending_import_notice")
      .then((pending) => {
        if (!pending) return;
        if (pending.ok && pending.payload) {
          setImportNotice({
            id: Date.now(),
            ok: true,
            msg: importSuccessMessage(pending.payload),
          });
        } else {
          setImportNotice({
            id: Date.now(),
            ok: false,
            msg: t("keys.importFailed", { error: pending.error || "unknown error" }),
          });
        }
        setTab("keys");
      })
      .catch(() => {});
    return () => {
      unlistenImported?.();
      unlistenImportError?.();
    };
  }, []);

  return (
    <div className="app">
      <Titlebar
        tab={tab}
        onTab={setTab}
        theme={theme}
        onToggleTheme={() =>
          setTheme((t) => (t === "light" ? "dark" : "light"))
        }
      />
      <main key={tab} className="view">
        {tab === "tools" && <ToolsView />}
        {tab === "keys" && <KeysView importNotice={importNotice} />}
        {tab === "usage" && <UsageView />}
        {tab === "sessions" && <SessionsView />}
        {tab === "playground" && <PlaygroundView />}
        {tab === "about" && <AboutView />}
      </main>
      <StatusBar />
    </div>
  );
}

function ToolsView() {
  const [activeId, setActiveId] = useState<ToolId>(TOOLS[0].id);
  const [model, setModel] = useState("");
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ApplyResult | null>(null);
  const [statuses, setStatuses] = useState<Record<string, ConfigStatus>>({});
  const [installs, setInstalls] = useState<Record<string, InstallStatus>>({});
  const [usage, setUsage] = useState<UsageInfo | null>(null);
  const [loadingUsage, setLoadingUsage] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [backups, setBackups] = useState<BackupEntry[]>([]);
  const [restoreBusy, setRestoreBusy] = useState(false);
  const [healthBusy, setHealthBusy] = useState(false);
  const [health, setHealth] = useState<HealthCheckReport | null>(null);
  const [unlockBusy, setUnlockBusy] = useState(false);
  const [unlockConfirmOpen, setUnlockConfirmOpen] = useState(false);
  const [unlockResult, setUnlockResult] = useState<{ ok: boolean; msg: string } | null>(null);
  const [syncHistoryBusy, setSyncHistoryBusy] = useState(false);
  const [rollbackHistoryBusy, setRollbackHistoryBusy] = useState(false);
  const [syncHistoryResult, setSyncHistoryResult] = useState<{ ok: boolean; msg: string } | null>(null);

  const active = useMemo<ToolMeta>(
    () => TOOLS.find((t) => t.id === activeId) ?? TOOLS[0],
    [activeId],
  );

  useEffect(() => {
    refresh();
  }, []);

  useEffect(() => {
    setError(null);
    setResult(null);
    setHealth(null);
    setUnlockResult(null);
    setSyncHistoryResult(null);
    const s = statuses[activeId];
    setModel(s?.configured && s.model ? s.model : "");
    loadBackups(activeId);
  }, [activeId, statuses]);

  useEffect(() => {
    const s = statuses[activeId];
    if (s?.configured && s.model && !model) {
      setModel(s.model);
    }
  }, [statuses]);


  async function loadBackups(tool: string) {
    try {
      const list = await invoke<BackupEntry[]>("list_config_backups", { tool });
      setBackups(list);
    } catch {
      setBackups([]);
    }
  }

  async function refresh() {
    try {
      const [s, i] = await Promise.all([
        invoke<ConfigStatus[]>("list_status"),
        invoke<InstallStatus[]>("list_installed"),
      ]);
      const sm: Record<string, ConfigStatus> = {};
      for (const x of s) sm[x.tool] = x;
      const im: Record<string, InstallStatus> = {};
      for (const x of i) im[x.tool] = x;
      setStatuses(sm);
      setInstalls(im);
      await loadBackups(activeId);

    } catch (e) {
      console.error(e);
    }
  }

  async function onFetchModels() {
    setLoadingModels(true);
    setError(null);
    try {
      const list = await invoke<ModelInfo[]>("fetch_models", { keyId: null });
      setModels(list);
      if (list.length > 0 && !model) setModel(list[0].id);
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setLoadingModels(false);
    }
  }

  function onUnlockPlugins() {
    setUnlockConfirmOpen(true);
  }

  async function runUnlockPlugins() {
    setUnlockConfirmOpen(false);
    setUnlockBusy(true);
    setError(null);
    setUnlockResult(null);
    try {
      const msg = await invoke<string>("unlock_codex_plugins");
      setUnlockResult({ ok: true, msg });
    } catch (e) {
      setUnlockResult({ ok: false, msg: humanizeError(String(e)) });
    } finally {
      setUnlockBusy(false);
    }
  }

  async function onSyncHistory() {
    setSyncHistoryBusy(true);
    setError(null);
    setSyncHistoryResult(null);
    try {
      const r = await invoke<SyncHistoryReport>("sync_codex_history");
      setSyncHistoryResult({
        ok: true,
        msg: t("tools.syncHistoryResult", {
          files: r.files_updated,
          rows: r.db_updated,
          provider: r.provider,
          backup: r.backup_dir || t("tools.syncHistoryNoBackup"),
        }),
      });
    } catch (e) {
      setSyncHistoryResult({ ok: false, msg: humanizeError(String(e)) });
    } finally {
      setSyncHistoryBusy(false);
    }
  }

  async function onRollbackHistory() {
    if (!confirm(t("tools.rollbackHistoryConfirm"))) return;
    setRollbackHistoryBusy(true);
    setError(null);
    setSyncHistoryResult(null);
    try {
      const r = await invoke<RollbackHistoryReport>("rollback_latest_codex_history_backup");
      setSyncHistoryResult({
        ok: true,
        msg: t("tools.rollbackHistoryResult", {
          backup: r.backup_dir,
          files: r.files_restored,
          db: r.db_restored ? t("dialog.yes") : t("dialog.no"),
        }),
      });
    } catch (e) {
      setSyncHistoryResult({ ok: false, msg: humanizeError(String(e)) });
    } finally {
      setRollbackHistoryBusy(false);
    }
  }

  async function onQueryUsage() {
    setLoadingUsage(true);
    setError(null);
    try {
      const u = await invoke<UsageInfo>("query_usage");
      setUsage(u);
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setLoadingUsage(false);
    }
  }

  async function onApply() {
    if (!model.trim()) {
      setError(t("tools.selectModelFirst"));
      return;
    }
    if (statuses[active.id]?.configured) {
      if (!confirm(t("tools.overwriteConfirm", { name: active.name }))) {
        return;
      }
    }
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const r = await invoke<ApplyResult>("apply_config", {
        tool: active.id,
        model: model.trim(),
      });
      setResult(r);
      await refresh();
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setBusy(false);
    }
  }

  async function onClear() {
    if (!confirm(t("tools.clearConfirm", { name: active.name }))) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      await invoke("clear_config", { tool: active.id });
      await refresh();
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setBusy(false);
    }
  }

  async function onLaunchCli() {
    setError(null);
    setLaunching(true);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const dir = await open({ directory: true, title: t("tools.selectDir") });
      if (!dir) {
        setLaunching(false);
        return;
      }
      await invoke("launch_cli_in_dir", { tool: active.id, dir });
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setLaunching(false);
    }
  }


  async function onRestoreBackup(path: string) {
    if (!confirm(t("tools.restoreConfirm"))) return;
    setRestoreBusy(true);
    setError(null);
    try {
      const r = await invoke<ApplyResult>("restore_config_backup", { tool: active.id, path });
      setResult(r);
      await refresh();
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setRestoreBusy(false);
    }
  }

  async function onDeleteBackup(path: string) {
    if (!confirm(t("tools.deleteBackupConfirm"))) return;
    setRestoreBusy(true);
    setError(null);
    try {
      await invoke("delete_config_backup", { path });
      await loadBackups(active.id);
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setRestoreBusy(false);
    }
  }

  async function onClearBackups() {
    if (!confirm(t("tools.clearBackupsConfirm", { name: active.name, count: backups.length }))) return;
    setRestoreBusy(true);
    setError(null);
    try {
      await invoke("clear_config_backups", { tool: active.id });
      await loadBackups(active.id);
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setRestoreBusy(false);
    }
  }

  async function onRollbackBackup() {
    if (!confirm(t("tools.rollbackConfirm"))) return;
    setRestoreBusy(true);
    setError(null);
    try {
      const r = await invoke<ApplyResult>("rollback_config_backup", { tool: active.id });
      setResult(r);
      await refresh();
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setRestoreBusy(false);
    }
  }

  async function onHealthCheck() {
    setHealthBusy(true);
    setError(null);
    setHealth(null);
    try {
      const report = await invoke<HealthCheckReport>("health_check", { tool: active.id });
      setHealth(report);
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setHealthBusy(false);
    }
  }

  function fmtBackupTime(ts: number): string {
    if (!ts) return "—";
    const d = new Date(ts * 1000);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }

  function isChatGptLogin(keyHint?: string | null): boolean {
    return !!keyHint && keyHint.toLowerCase().includes("chatgpt");
  }

  function currentConfigItems(s: ConfigStatus) {
    if (isChatGptLogin(s.key_hint) && !s.base_url && !s.model) {
      return [{ label: t("tools.key"), value: s.key_hint! }];
    }
    return [
      { label: t("tools.address"), value: s.base_url },
      { label: t("tools.key"), value: s.key_hint },
      { label: t("tools.model"), value: s.model },
    ].filter((item) => item.value && item.value.trim());
  }

  function backupSummaryItems(backup: BackupEntry) {
    if (isChatGptLogin(backup.key_hint) && !backup.base_url && !backup.model) {
      return [{ label: t("tools.key"), value: backup.key_hint! }];
    }
    return [
      { label: t("tools.model"), value: backup.model },
      { label: t("tools.address"), value: backup.base_url },
      { label: t("tools.key"), value: backup.key_hint },
    ].filter((item) => item.value && item.value.trim());
  }

  return (
    <div className="body">
      <aside className="sidebar">
        <div className="section-label">{t("tools.section")}</div>
        {TOOLS.map((t_tool, i) => {
          const s = statuses[t_tool.id];
          const inst = installs[t_tool.id];
          return (
            <div
              key={t_tool.id}
              className={`tool-item ${activeId === t_tool.id ? "active" : ""}`}
              style={{ animationDelay: `${i * 35}ms` }}
              onClick={() => setActiveId(t_tool.id)}
            >
              <div className="tool-icon">
                <ToolIcon id={t_tool.id} size={20} />
              </div>
              <div className="name-col">
                <div className="name">{t_tool.name}</div>
                <div className={`install-line ${inst?.installed ? "yes" : ""}`}>
                  {inst ? (inst.installed ? t("tools.installed") : t("tools.notDetected")) : t("tools.checking")}
                </div>
              </div>
              <div
                className={`status-dot ${s?.configured ? "on" : ""}`}
                title={s?.configured ? t("tools.connected") : t("tools.notConnected")}
              />
            </div>
          );
        })}
      </aside>

      <section className="panel view-enter">
        <header className="panel-header">
          <div className="big-mark svg-mark">
            <ToolIcon id={active.id} size={20} />
          </div>
          <div style={{ flex: 1 }}>
            <h2>
              {active.name}
              {statuses[active.id]?.configured && (
                <span className="pill ok">{t("tools.connected")}</span>
              )}
              {installs[active.id] && !installs[active.id].installed && (
                <span className="pill warn">{t("tools.notInstalled")}</span>
              )}
            </h2>
            <div className="sub tool-detail-desc">{t(`tools.desc.${active.id}`)}</div>
          </div>
        </header>

        <div className="panel-divider" />

        {!statuses[active.id]?.configured && (
          <div className="empty compact">
            <div>{t("tools.emptyConfig")}</div>
            <div className="hint">{t("tools.emptyConfigHint")}</div>
          </div>
        )}

        {statuses[active.id]?.configured && (
          <div className="config-card">
            <div className="config-title">{t("tools.currentConfig")}</div>
            <div className="config-items">
              {currentConfigItems(statuses[active.id]!).map((item) => (
                <div className="config-row" key={item.label}>
                  <span className="config-label">{item.label}</span>
                  <code>{item.value}</code>
                </div>
              ))}
            </div>
          </div>
        )}

        <div className="group">
          <div className="field">
            <label>{t("tools.model")}</label>
            <div className="control">
              <div className="input-row">
                <select
                  className="select"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  disabled={models.length === 0}
                >
                  {models.length === 0 ? (
                    <option value={model || ""}>{model || t("tools.selectModel")}</option>
                  ) : (
                    models.map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.id}
                        {m.owned_by ? ` · ${m.owned_by}` : ""}
                      </option>
                    ))
                  )}
                </select>
                <button
                  className="btn"
                  onClick={onFetchModels}
                  disabled={loadingModels}
                >
                  {loadingModels ? (
                    <>
                      <span className="spinner" /> {t("tools.fetching")}
                    </>
                  ) : (
                    t("tools.fetchModels")
                  )}
                </button>
              </div>
            </div>
          </div>
        </div>




        {backups.length > 0 && (
          <div className="files-card">
            <div className="backup-card-head">
              <h4>{t("tools.backups")}</h4>
              <div className="backup-actions">
                <button
                  className="btn small"
                  disabled={restoreBusy}
                  onClick={onRollbackBackup}
                >
                  {t("tools.rollbackLatest")}
                </button>
                <button
                  className="btn small danger"
                  disabled={restoreBusy}
                  onClick={onClearBackups}
                >
                  {t("tools.clearBackups")}
                </button>
              </div>
            </div>
            <div className="backup-list">
              {backups.slice(0, 3).map((backup) => (
                <div className="backup-item" key={backup.path}>
                  <div>
                    <div className="mono">{backup.name}</div>
                    <div className="backup-summary-grid">
                      {backupSummaryItems(backup).map((item) => (
                        <span key={item.label}>{item.label}: <code>{item.value}</code></span>
                      ))}
                    </div>
                    <div className="hint">{fmtBackupTime(backup.modified)} · {backup.files.join(", ")}</div>
                  </div>
                  <div className="backup-actions">
                    <button
                      className="btn small"
                      disabled={restoreBusy}
                      onClick={() => onRestoreBackup(backup.path)}
                    >
                      {t("tools.restore")}
                    </button>
                    <button
                      className="btn small danger"
                      disabled={restoreBusy}
                      onClick={() => onDeleteBackup(backup.path)}
                    >
                      {t("tools.deleteBackup")}
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        <div className="actions flex-col">
          <div className="actions-row">
            <button className="btn primary" onClick={onApply} disabled={busy}>
              {busy ? (
                <>
                  <span className="spinner" /> {t("tools.writing")}
                </>
              ) : statuses[active.id]?.configured ? (
                t("tools.update")
              ) : (
                t("tools.apply")
              )}
            </button>
            {statuses[active.id]?.configured && (
              <button className="btn danger" onClick={onClear} disabled={busy}>
                {t("tools.clear")}
              </button>
            )}
            {installs[active.id]?.installed && active.id !== "openclaw" && (
              <button className="btn" onClick={onLaunchCli} disabled={launching}>
                {launching ? (
                  <>
                    <span className="spinner" /> {t("tools.launching")}
                  </>
                ) : (
                  t("tools.launchCli")
                )}
              </button>
            )}
            {active.id === "codex" && (
              <>
                <button
                  className="btn"
                  onClick={onUnlockPlugins}
                  disabled={unlockBusy}
                  title={t("tools.unlockPluginsDesc")}
                >
                  {unlockBusy ? (
                    <>
                      <span className="spinner" /> {t("tools.unlocking")}
                    </>
                  ) : (
                    <>
                      {t("tools.unlockPlugins")}
                    </>
                  )}
                </button>
                <button
                  className="btn"
                  onClick={onSyncHistory}
                  disabled={syncHistoryBusy || rollbackHistoryBusy}
                  title={t("tools.syncHistoryDesc")}
                >
                  {syncHistoryBusy ? (
                    <>
                      <span className="spinner" /> {t("tools.syncingHistory")}
                    </>
                  ) : (
                    <>
                      {t("tools.syncHistory")}
                    </>
                  )}
                </button>
                <button
                  className="btn ghost"
                  onClick={onRollbackHistory}
                  disabled={syncHistoryBusy || rollbackHistoryBusy}
                >
                  {rollbackHistoryBusy ? t("tools.rollingBackHistory") : t("tools.rollbackHistory")}
                </button>
              </>
            )}
          </div>
          <div className="actions-row secondary-row">
            <button className="btn" onClick={onQueryUsage} disabled={loadingUsage}>
              {loadingUsage ? (
                <>
                  <span className="spinner" /> {t("tools.querying")}
                </>
              ) : (
                t("tools.queryUsage")
              )}
            </button>
            <button className="btn" onClick={onHealthCheck} disabled={healthBusy || !statuses[active.id]?.configured}>
              {healthBusy ? (
                <>
                  <span className="spinner" /> {t("tools.healthChecking")}
                </>
              ) : (
                t("tools.healthCheck")
              )}
            </button>
            <button className="btn ghost" onClick={refresh} disabled={busy}>
              {t("tools.refresh")}
            </button>
          </div>
        </div>

        {health && (
          <div className={`alert ${health.ok ? "ok" : "err"} toast-in`}>
            <div style={{ flex: 1 }}>
              <strong>
                {health.ok ? t("tools.healthOk") : t("tools.healthFailed")}
              </strong>
              <div className="hint" style={{ marginTop: 4 }}>
                {health.model} · {health.elapsed_ms}ms · {health.base_url}
              </div>
              <ul style={{ margin: "8px 0 0 18px", padding: 0 }}>
                {health.steps.map((step) => (
                  <li key={step.name}>
                    <span className={step.ok ? "usage-ok" : "usage-bad"}>
                      {step.ok ? "✓" : "✗"}
                    </span>{" "}
                    <code>{step.name}</code>: {step.detail}
                  </li>
                ))}
              </ul>
            </div>
          </div>
        )}

        {usage && (
          <div className="usage-card toast-in">
            {usage.key_name && (
              <div className="usage-row usage-card-full-row">
                <span className="usage-label">{t("keys.name")}</span>
                <span className="usage-value">{usage.key_name}</span>
              </div>
            )}
            {usage.key_prefix && (
              <div className="usage-row">
                <span className="usage-label">{t("logs.key")}</span>
                <span className="usage-value mono">{usage.key_prefix}</span>
              </div>
            )}
            {usage.plan_name && (
              <div className="usage-row">
                <span className="usage-label">{t("usage.planName")}</span>
                <span className="usage-value">{usage.plan_name}</span>
              </div>
            )}
            <div className="usage-row">
              <span className="usage-label">{t("tools.accountStatus")}</span>
              <span className={usage.is_valid ? "usage-ok" : "usage-bad"}>
                {usage.is_valid ? t("tools.statusOk") : t("tools.statusExpired")}
              </span>
            </div>
            {!usage.is_valid && usage.invalid_message && (
              <div className="usage-row usage-card-full-row">
                <span className="usage-label">{t("usage.invalidReason")}</span>
                <span className="usage-bad usage-reason">{usage.invalid_message}</span>
              </div>
            )}
            <div className="usage-row">
              <span className="usage-label">{t("tools.balance")}</span>
              <span className="usage-value highlight">
                {usage.remaining !== null
                  ? `${usage.remaining.toFixed(4)} ${usage.unit}`
                  : "\u2014"}
              </span>
            </div>
            {usage.total !== undefined && usage.total !== null && (
              <div className="usage-row">
                <span className="usage-label">{t("usage.totalCredit")}</span>
                <span className="usage-value">
                  {usage.total.toFixed(4)} {usage.unit}
                </span>
              </div>
            )}
            {usage.used !== undefined && usage.used !== null && (
              <div className="usage-row">
                <span className="usage-label">{t("usage.estimatedCost")}</span>
                <span className="usage-value">
                  {usage.used.toFixed(4)} {usage.unit}
                </span>
              </div>
            )}
            {usage.expires_at && (
              <div className="usage-row usage-card-full-row">
                <span className="usage-label">{t("usage.expiresAt")}</span>
                <span className="usage-value">{new Date(usage.expires_at).toLocaleString()}</span>
              </div>
            )}
          </div>
        )}

        {unlockResult && (
          <div className={`alert ${unlockResult.ok ? "ok" : "err"} toast-in`}>
            <div style={{ flex: 1 }}>
              <strong>
                {unlockResult.ok ? t("tools.unlockSuccess") : t("tools.unlockFailed")}
              </strong>
              <div style={{ marginTop: 4 }}>{unlockResult.msg}</div>
            </div>
          </div>
        )}

        {syncHistoryResult && (
          <div className={`alert ${syncHistoryResult.ok ? "ok" : "err"} toast-in`}>
            <div style={{ flex: 1 }}>
              <strong>
                {syncHistoryResult.ok ? t("tools.syncHistorySuccess") : t("tools.syncHistoryFailed")}
              </strong>
              <div style={{ marginTop: 4 }}>{syncHistoryResult.msg}</div>
            </div>
          </div>
        )}


        <ConfirmDialog
          open={unlockConfirmOpen}
          title={t("tools.unlockPlugins")}
          message={t("tools.unlockPluginsConfirm")}
          confirmLabel={t("tools.unlockPlugins")}
          cancelLabel={t("dialog.cancel")}
          danger
          onConfirm={runUnlockPlugins}
          onCancel={() => setUnlockConfirmOpen(false)}
        />

        {error && (
          <div className="alert err toast-in">
            <div style={{ flex: 1 }}>
              <strong>{t("tools.failed")}</strong>
              <pre>{error}</pre>
            </div>
          </div>
        )}

        {result && (
          <>
            <div className="alert ok toast-in">
              <div style={{ flex: 1 }}>
                <strong>{t("tools.configWritten")}</strong>
                <div style={{ marginTop: 2 }}>
                  {t("tools.configWrittenDesc", { count: result.files.length })}
                </div>
              </div>
            </div>
            <div className="files-card">
              <h4>{t("tools.written")}</h4>
              <ul>
                {result.files.map((f) => (
                  <li key={f}>{f}</li>
                ))}
              </ul>
              {result.backup_dir && (
                <div className="backup-line">
                  {t("tools.backupAt")}<code>{result.backup_dir}</code>
                </div>
              )}
            </div>
          </>
        )}
      </section>
    </div>
  );
}
