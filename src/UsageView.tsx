import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

interface UsageRecord {
  ts: number;
  tool: string;
  session: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  path: string;
}

interface UsageSnapshot {
  files_scanned: number;
  records: UsageRecord[];
}

interface SessionUsage {
  tool: string;
  session: string;
  model: string;
  path: string;
  ts: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

type RangeKey = "24h" | "7d" | "30d" | "all";
type ToolFilter = "all" | "claude-code" | "codex";

const RANGES: Array<{ key: RangeKey; label: string; hours: number | null }> = [
  { key: "24h", label: t("usage.range24h"), hours: 24 },
  { key: "7d", label: t("usage.range7d"), hours: 24 * 7 },
  { key: "30d", label: t("usage.range30d"), hours: 24 * 30 },
  { key: "all", label: t("usage.rangeAll"), hours: null },
];

function fmtNum(v: number): string {
  if (v >= 1_000_000_000) return `${(v / 1_000_000_000).toFixed(2)}B`;
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`;
  if (v >= 1_000) return `${(v / 1_000).toFixed(1)}K`;
  return `${v}`;
}

function fmtDate(ts: number): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function fmtShortDate(ts: number): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function fmtBucket(ts: number, kind: "hour" | "day"): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  if (kind === "hour") {
    return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:00`;
  }
  return fmtShortDate(ts);
}

function totalTokens(r: UsageRecord): number {
  return r.input_tokens + r.output_tokens + r.cache_read_tokens + r.cache_write_tokens;
}

function totalSessionTokens(r: SessionUsage): number {
  return r.input_tokens + r.output_tokens + r.cache_read_tokens + r.cache_write_tokens;
}

function toolLabel(tool: string): string {
  switch (tool) {
    case "claude-code":
      return t("usage.claudeCode");
    case "codex":
      return t("usage.codex");
    default:
      return tool;
  }
}

function modelLabel(model: string): string {
  return model && model.trim() ? model : "—";
}

function topEntries<T>(items: T[], limit: number, score: (item: T) => number): T[] {
  return [...items].sort((a, b) => score(b) - score(a)).slice(0, limit);
}

export default function UsageView() {
  const [snapshot, setSnapshot] = useState<UsageSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [range, setRange] = useState<RangeKey>("7d");
  const [toolFilter, setToolFilter] = useState<ToolFilter>("all");
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 60_000);
    return () => clearInterval(timer);
  }, []);

  async function refresh() {
    setLoading(true);
    setError(null);
    try {
      const data = await invoke<UsageSnapshot>("usage_snapshot");
      setSnapshot(data);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }


  async function onExportCsv() {
    try {
      const csv = await invoke<string>("export_usage_csv");
      const { save } = await import("@tauri-apps/plugin-dialog");
      const filePath = await save({
        defaultPath: `beeapi-usage-${new Date().toISOString().slice(0, 10)}.csv`,
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!filePath) return;
      const { writeTextFile } = await import("@tauri-apps/plugin-fs");
      await writeTextFile(filePath, csv);
      setNotice(t("usage.exported"));
      setTimeout(() => setNotice(null), 2200);
    } catch (e) {
      setError(String(e));
    }
  }

  const filtered = useMemo(() => {
    const records = snapshot?.records ?? [];
    const rangeDef = RANGES.find((r) => r.key === range) ?? RANGES[1];
    const cutoff = rangeDef.hours === null ? null : Date.now() - rangeDef.hours * 3600 * 1000;
    return records.filter((r) => {
      if (toolFilter !== "all" && r.tool !== toolFilter) return false;
      if (cutoff !== null && r.ts * 1000 < cutoff) return false;
      return true;
    });
  }, [snapshot, range, toolFilter]);

  const summary = useMemo(() => {
    const sessions = new Set<string>();
    let input = 0;
    let output = 0;
    let cache = 0;
    let latest = 0;
    for (const r of filtered) {
      sessions.add(r.path);
      input += r.input_tokens;
      output += r.output_tokens;
      cache += r.cache_read_tokens + r.cache_write_tokens;
      latest = Math.max(latest, r.ts);
    }
    return {
      records: filtered.length,
      sessions: sessions.size,
      input,
      output,
      cache,
      total: input + output + cache,
      latest,
    };
  }, [filtered]);

  const sessionStats = useMemo(() => {
    const map = new Map<string, SessionUsage>();
    for (const r of filtered) {
      const current = map.get(r.path);
      if (!current) {
        map.set(r.path, {
          tool: r.tool,
          session: r.session,
          model: r.model,
          path: r.path,
          ts: r.ts,
          input_tokens: r.input_tokens,
          output_tokens: r.output_tokens,
          cache_read_tokens: r.cache_read_tokens,
          cache_write_tokens: r.cache_write_tokens,
        });
        continue;
      }

      const isLatest = r.ts >= current.ts;
      current.ts = Math.max(current.ts, r.ts);
      current.input_tokens += r.input_tokens;
      current.output_tokens += r.output_tokens;
      current.cache_read_tokens += r.cache_read_tokens;
      current.cache_write_tokens += r.cache_write_tokens;
      if (isLatest) {
        current.session = r.session;
        current.model = r.model;
        current.tool = r.tool;
      }
    }
    return [...map.values()].sort((a, b) => b.ts - a.ts);
  }, [filtered]);

  const toolStats = useMemo(() => {
    const map = new Map<string, { tool: string; count: number; tokens: number }>();
    for (const r of sessionStats) {
      const current = map.get(r.tool) ?? { tool: r.tool, count: 0, tokens: 0 };
      current.count += 1;
      current.tokens += totalSessionTokens(r);
      map.set(r.tool, current);
    }
    return topEntries([...map.values()], 10, (item) => item.tokens);
  }, [sessionStats]);

  const modelStats = useMemo(() => {
    const map = new Map<string, { model: string; count: number; tokens: number }>();
    for (const r of sessionStats) {
      const key = r.model || "-";
      const current = map.get(key) ?? { model: key, count: 0, tokens: 0 };
      current.count += 1;
      current.tokens += totalSessionTokens(r);
      map.set(key, current);
    }
    return topEntries([...map.values()], 10, (item) => item.tokens);
  }, [sessionStats]);

  const trend = useMemo(() => {
    const kind: "hour" | "day" = range === "24h" ? "hour" : "day";
    const map = new Map<string, { label: string; count: number; tokens: number }>();
    for (const r of [...sessionStats].sort((a, b) => a.ts - b.ts)) {
      const d = new Date(r.ts * 1000);
      const key =
        kind === "hour"
          ? `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}-${d.getHours()}`
          : `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
      const label = fmtBucket(r.ts, kind);
      const current = map.get(key) ?? { label, count: 0, tokens: 0 };
      current.count += 1;
      current.tokens += totalSessionTokens(r);
      map.set(key, current);
    }
    return [...map.values()].slice(-14);
  }, [sessionStats, range]);

  const recent = useMemo(() => sessionStats.slice(0, 12), [sessionStats]);
  const maxTrend = Math.max(1, ...trend.map((x) => x.tokens));
  const maxTool = Math.max(1, ...toolStats.map((x) => x.tokens));
  const maxModel = Math.max(1, ...modelStats.map((x) => x.tokens));

  return (
    <div className="panel view-enter usage-view">
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
            <path d="M4 20V10" />
            <path d="M10 20V4" />
            <path d="M16 20v-7" />
            <path d="M22 20H2" />
          </svg>
        </div>
        <div style={{ flex: 1 }}>
          <h2>
            {t("usage.title")}
            <span className="pill">{summary.records}</span>
            <span className="pill">{summary.sessions}</span>
          </h2>
          <div className="sub">{t("usage.desc")}</div>
        </div>
      </header>

      <div className="panel-divider" />

      <div className="usage-toolbar">
        <div className="segmented">
          {RANGES.map((item) => (
            <button
              key={item.key}
              className={`segmented-btn ${range === item.key ? "active" : ""}`}
              onClick={() => setRange(item.key)}
            >
              {item.label}
            </button>
          ))}
        </div>
        <div className="segmented">
          <button
            className={`segmented-btn ${toolFilter === "all" ? "active" : ""}`}
            onClick={() => setToolFilter("all")}
          >
            {t("usage.allSources")}
          </button>
          <button
            className={`segmented-btn ${toolFilter === "claude-code" ? "active" : ""}`}
            onClick={() => setToolFilter("claude-code")}
          >
            {t("usage.claudeCode")}
          </button>
          <button
            className={`segmented-btn ${toolFilter === "codex" ? "active" : ""}`}
            onClick={() => setToolFilter("codex")}
          >
            {t("usage.codex")}
          </button>
        </div>
        <div className="spacer" />
        <button className="btn small" onClick={onExportCsv} disabled={!snapshot || snapshot.records.length === 0}>
          {t("usage.exportCsv")}
        </button>
        <button className="btn small" onClick={refresh} disabled={loading}>
          {loading ? <span className="spinner" /> : null}
          {t("usage.refresh")}
        </button>
      </div>

      <div className="usage-note">{t("usage.sourceHint")}</div>
      {notice && <div className="alert ok toast-in" style={{ marginTop: 10 }}>{notice}</div>}

      {error ? (
        <div className="alert err toast-in" style={{ marginTop: 14 }}>
          <div style={{ flex: 1 }}>
            <strong>{t("usage.failed")}</strong>
            <pre>{error}</pre>
          </div>
        </div>
      ) : null}

      {!loading && filtered.length === 0 ? (
        <div className="usage-empty">{t("usage.empty")}</div>
      ) : null}

      <div className="metric-grid" style={{ marginTop: 14 }}>
        <div className="metric-card">
          <div className="metric-label">{t("usage.records")}</div>
          <div className="metric-value">{fmtNum(summary.records)}</div>
          <div className="metric-sub">
            {toolFilter === "all" ? t("usage.allSources") : toolLabel(toolFilter)}
          </div>
        </div>
        <div className="metric-card">
          <div className="metric-label">{t("usage.files")}</div>
          <div className="metric-value">{fmtNum(snapshot?.files_scanned ?? 0)}</div>
          <div className="metric-sub">
            {t("usage.sessions")} {fmtNum(summary.sessions)}
          </div>
        </div>
        <div className="metric-card">
          <div className="metric-label">{t("usage.totalTokens")}</div>
          <div className="metric-value">{fmtNum(summary.total)}</div>
          <div className="metric-sub">
            {t("usage.inputTokens")} {fmtNum(summary.input)} · {t("usage.outputTokens")} {fmtNum(summary.output)} · {t("usage.cacheTokens")} {fmtNum(summary.cache)}
          </div>
        </div>
        <div className="metric-card">
          <div className="metric-label">{t("usage.avgPerSession")}</div>
          <div className="metric-value">{fmtNum(summary.sessions ? Math.round(summary.total / summary.sessions) : 0)}</div>
          <div className="metric-sub">{t("usage.rangeTokens")}</div>
        </div>
        <div className="metric-card">
          <div className="metric-label">{t("usage.latest")}</div>
          <div className="metric-value">{summary.latest ? fmtDate(summary.latest) : "—"}</div>
          <div className="metric-sub">{summary.latest ? t("usage.recentLogs") : t("usage.empty")}</div>
        </div>
      </div>

      <div className="usage-panel" style={{ marginTop: 12 }}>
        <div className="usage-panel-head">
          <h3>{t("usage.trend")}</h3>
          <span>{range === "24h" ? t("usage.hourly") : t("usage.daily")}</span>
        </div>
        {trend.length > 0 ? (
          <div className="usage-bars">
            {trend.map((item) => (
              <div key={item.label} className="usage-bar-row">
                <div className="usage-bar-label">{item.label}</div>
                <div className="usage-bar-track">
                  <div
                    className="usage-bar-fill"
                    style={{ width: `${(item.tokens / maxTrend) * 100}%` }}
                  />
                </div>
                <div className="usage-bar-meta">
                  {fmtNum(item.count)} / {fmtNum(item.tokens)}
                </div>
              </div>
            ))}
          </div>
        ) : (
          <div className="usage-empty">{t("usage.empty")}</div>
        )}
      </div>

      <div className="usage-subgrid" style={{ marginTop: 12 }}>
        <div className="usage-panel">
          <div className="usage-panel-head">
            <h3>{t("usage.toolBreakdown")}</h3>
            <span>{fmtNum(toolStats.length)}</span>
          </div>
          {toolStats.length > 0 ? (
            <div className="usage-bars">
              {toolStats.map((item) => (
                <div key={item.tool} className="usage-bar-row">
                  <div className="usage-bar-label">{toolLabel(item.tool)}</div>
                  <div className="usage-bar-track">
                    <div
                      className="usage-bar-fill"
                      style={{ width: `${(item.tokens / maxTool) * 100}%` }}
                    />
                  </div>
                  <div className="usage-bar-meta">
                    {fmtNum(item.count)} / {fmtNum(item.tokens)}
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div className="usage-empty">{t("usage.empty")}</div>
          )}
        </div>

        <div className="usage-panel">
          <div className="usage-panel-head">
            <h3>{t("usage.modelBreakdown")}</h3>
            <span>{fmtNum(modelStats.length)}</span>
          </div>
          {modelStats.length > 0 ? (
            <div className="usage-bars">
              {modelStats.map((item) => (
                <div key={item.model} className="usage-bar-row">
                  <div className="usage-bar-label" title={item.model}>
                    {modelLabel(item.model)}
                  </div>
                  <div className="usage-bar-track">
                    <div
                      className="usage-bar-fill"
                      style={{ width: `${(item.tokens / maxModel) * 100}%` }}
                    />
                  </div>
                  <div className="usage-bar-meta">
                    {fmtNum(item.count)} / {fmtNum(item.tokens)}
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div className="usage-empty">{t("usage.empty")}</div>
          )}
        </div>
      </div>

      <div className="usage-panel" style={{ marginTop: 12 }}>
        <div className="usage-panel-head">
          <h3>{t("usage.recentLogs")}</h3>
          <span>{fmtNum(recent.length)}</span>
        </div>
        {recent.length > 0 ? (
          <table className="log-table usage-table">
            <thead>
              <tr>
                <th>{t("usage.time")}</th>
                <th>{t("usage.source")}</th>
                <th>{t("usage.model")}</th>
                <th>{t("usage.status")}</th>
                <th>{t("usage.path")}</th>
              </tr>
            </thead>
            <tbody>
              {recent.map((r) => (
                <tr key={`${r.path}-${r.ts}-${r.model}`} className="log-row">
                  <td className="mono">{fmtDate(r.ts)}</td>
                  <td>{toolLabel(r.tool)}</td>
                  <td title={r.model}>{modelLabel(r.model)}</td>
                  <td className="mono">{fmtNum(totalTokens(r))}</td>
                  <td title={r.session} style={{ maxWidth: 360, overflow: "hidden", textOverflow: "ellipsis" }}>
                    {r.session}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="usage-empty">{t("usage.empty")}</div>
        )}
      </div>
    </div>
  );
}
