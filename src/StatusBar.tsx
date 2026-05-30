import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";


interface RequestLogEntry {
  id: number;
  ts: number;
  method: string;
  path: string;
  model: string | null;
  status: number;
  key_label: string;
  key_id: string;
  latency_ms: number;
  retries: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  usage_recorded: boolean;
  upstream_error: string | null;
}

function fmtTime(ts: number): string {
  if (!ts) return "?";
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function totalTokens(log: RequestLogEntry): number {
  return log.input_tokens + log.output_tokens + log.cache_read_tokens + log.cache_write_tokens;
}

interface ProxyInfo {
  pool_enabled: boolean;
  upstream: string;
  local_base: string;
}

interface KeyStat {
  id: string;
  label: string;
  enabled: boolean;
  success: number;
  failure: number;
  last_status: number | null;
  cooling_until: number | null;
  last_error: string | null;
  secret_tail: string;
}

export default function StatusBar() {
  const [proxyOk, setProxyOk] = useState<boolean | null>(null);
  const [poolEnabled, setPoolEnabled] = useState(false);
  const [keyCount, setKeyCount] = useState(0);
  const [totalSuccess, setTotalSuccess] = useState(0);
  const [totalFailure, setTotalFailure] = useState(0);
  const [logs, setLogs] = useState<RequestLogEntry[]>([]);
  const [showLogs, setShowLogs] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, []);

  async function refresh() {
    try {
      const ok = await invoke<boolean>("proxy_health");
      setProxyOk(ok);
      const info = await invoke<ProxyInfo>("proxy_info");
      setPoolEnabled(info.pool_enabled);
      const stats = await invoke<KeyStat[]>("pool_stats");
      setKeyCount(stats.filter((k) => k.enabled).length);
      let succ = 0;
      let fail = 0;
      for (const stat of stats) {
        succ += stat.success;
        fail += stat.failure;
      }
      setTotalSuccess(succ);
      setTotalFailure(fail);
      setLogs(await invoke<RequestLogEntry[]>("request_log"));
    } catch {
      setProxyOk(false);
    }
  }


  async function exportLogs() {
    try {
      const csv = await invoke<string>("export_request_log_csv");
      const { save } = await import("@tauri-apps/plugin-dialog");
      const filePath = await save({
        defaultPath: `beeapi-request-log-${new Date().toISOString().slice(0, 10)}.csv`,
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!filePath) return;
      const { writeTextFile } = await import("@tauri-apps/plugin-fs");
      await writeTextFile(filePath, csv);
      setNotice(t("logs.exported"));
      setTimeout(() => setNotice(null), 1800);
    } catch {
      setNotice(t("logs.exportFailed"));
      setTimeout(() => setNotice(null), 1800);
    }
  }

  async function clearLogs() {
    await invoke("clear_request_log");
    setLogs([]);
  }

  const recentLogs = useMemo(() => [...logs].slice(-12).reverse(), [logs]);

  if (!poolEnabled) return null;

  return (
    <div className="statusbar">
      <div className="statusbar-left">
        <span className="statusbar-item">
          <span
            className={`statusbar-dot ${proxyOk === true ? "ok" : proxyOk === false ? "err" : ""}`}
          />
          <span>
            {proxyOk === true
              ? t("statusbar.proxyOk")
              : proxyOk === false
                ? t("statusbar.proxyErr")
                : t("statusbar.checking")}
          </span>
        </span>

        <span className="statusbar-item">
          {t("statusbar.keys", { count: keyCount })}
        </span>

        {(totalSuccess > 0 || totalFailure > 0) && (
          <span className="statusbar-item">
            <span className="statusbar-success">{totalSuccess}</span>
            {" / "}
            <span className="statusbar-failure">{totalFailure}</span>
            {" " + t("statusbar.requests")}
          </span>
        )}
      </div>

      <div className="statusbar-right">
        <button className="statusbar-log-btn" onClick={() => setShowLogs((v) => !v)}>
          {t("logs.title")} ({logs.length})
        </button>
        <span className="statusbar-item">
          {t("statusbar.poolOn")}
        </span>
      </div>

      {showLogs && (
        <div className="request-log-popover">
          <div className="request-log-head">
            <strong>{t("logs.title")}</strong>
            <div className="request-log-actions">
              <button className="btn small" onClick={exportLogs}>{t("logs.export")}</button>
              <button className="btn small danger" onClick={clearLogs}>{t("logs.clear")}</button>
            </div>
          </div>
          {recentLogs.length === 0 ? (
            <div className="empty compact">{t("logs.empty")}</div>
          ) : (
            <table className="log-table compact-log-table">
              <thead>
                <tr>
                  <th>{t("logs.time")}</th>
                  <th>{t("logs.model")}</th>
                  <th>{t("logs.status")}</th>
                  <th>{t("logs.key")}</th>
                  <th>{t("logs.latency")}</th>
                  <th>{t("logs.tokens")}</th>
                  <th>{t("logs.error")}</th>
                </tr>
              </thead>
              <tbody>
                {recentLogs.map((log) => (
                  <tr key={log.id} className="log-row">
                    <td className="mono">{fmtTime(log.ts)}</td>
                    <td title={log.path}>{log.model || "?"}</td>
                    <td className={log.status >= 400 ? "usage-bad" : "usage-ok"}>{log.status}</td>
                    <td>{log.key_label || "?"}</td>
                    <td className="mono">{log.latency_ms}ms</td>
                    <td className="mono">{totalTokens(log)}</td>
                    <td title={log.upstream_error || ""}>{log.upstream_error || (log.retries ? `retry ${log.retries}` : "?")}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {notice && <div className="hint" style={{ marginTop: 8 }}>{notice}</div>}
        </div>
      )}
    </div>
  );
}
