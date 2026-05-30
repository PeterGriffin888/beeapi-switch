import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { humanizeError } from "./errors";
import { t } from "./i18n";

interface ApiKey {
  id: string;
  secret: string;
  label: string;
  enabled: boolean;
  weight: number;
}

interface KeyStat {
  id: string;
  label: string;
  enabled: boolean;
  weight: number;
  success: number;
  failure: number;
  last_status: number | null;
  cooling_until: number | null;
  cooldown_stage: number;
  last_error: string | null;
  secret_tail: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  estimated_cost: number;
}

interface ProxyInfo {
  local_base: string;
  lan_base: string | null;
  upstream: string;
  port: number;
  token: string;
  lan: boolean;
  pool_enabled: boolean;
  primary_key_id: string | null;
  fail_threshold: number;
}

function randomId() {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
}

export default function KeysView({ importNotice }: { importNotice?: { id: number; ok: boolean; msg: string } | null }) {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [stats, setStats] = useState<Record<string, KeyStat>>({});
  const [proxyInfo, setProxyInfo] = useState<ProxyInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [newLabel, setNewLabel] = useState("");
  const [newSecret, setNewSecret] = useState("");
  const [showToken, setShowToken] = useState(false);

  useEffect(() => {
    loadAll();
    const timer = setInterval(refreshStats, 2500);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!importNotice) return;
    loadAll();
    if (importNotice.ok) {
      flash(importNotice.msg);
      setError(null);
    } else {
      setError(importNotice.msg);
    }
  }, [importNotice?.id]);

  async function loadAll() {
    try {
      const info = await invoke<ProxyInfo>("proxy_info");
      setProxyInfo(info);
      const pool = await invoke<{
        keys: ApiKey[];
        upstream: string;
        pool_enabled: boolean;
        primary_key_id: string | null;
        fail_threshold: number;
      }>("load_pool");
      setKeys(pool.keys.map((k) => ({ ...k, weight: k.weight || 1 })));
      await refreshStats();
    } catch (e) {
      setError(humanizeError(String(e)));
    }
  }

  async function refreshStats() {
    try {
      const list = await invoke<KeyStat[]>("pool_stats");
      const map: Record<string, KeyStat> = {};
      for (const s of list) map[s.id] = s;
      setStats(map);
    } catch {
      // best-effort
    }
  }

  function flash(msg: string) {
    setNotice(msg);
    setTimeout(() => setNotice(null), 1800);
  }

  async function persistKeys(next: ApiKey[]) {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_pool_keys", { keys: next });
      setKeys(next);
      flash(t("keys.saved"));
      await refreshStats();
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setBusy(false);
    }
  }

  function onAdd() {
    if (!newSecret.trim()) {
      setError(t("keys.enterKey"));
      return;
    }
    setBusy(true);
    setError(null);
    invoke<number>("verify_key", { secret: newSecret.trim() })
      .then((count) => {
        const entry: ApiKey = {
          id: randomId(),
          secret: newSecret.trim(),
          label: newLabel.trim() || `key-${keys.length + 1}`,
          enabled: true,
          weight: 1,
        };
        const next = [...keys, entry];
        setNewLabel("");
        setNewSecret("");
        flash(t("keys.keyValid", { count }));
        persistKeys(next);
      })
      .catch((e) => {
        const msg = humanizeError(String(e));
        if (confirm(t("keys.verifyFailed", { msg }))) {
          const entry: ApiKey = {
            id: randomId(),
            secret: newSecret.trim(),
            label: newLabel.trim() || `key-${keys.length + 1}`,
            enabled: true,
            weight: 1,
          };
          const next = [...keys, entry];
          setNewLabel("");
          setNewSecret("");
          persistKeys(next);
        }
      })
      .finally(() => setBusy(false));
  }


  async function onEnableDeepLink() {
    if (!confirm(t("keys.enableDeepLinkConfirm"))) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("register_deep_link_service");
      flash(t("keys.enableDeepLinkOk"));
    } catch (e) {
      setError(humanizeError(String(e)));
    } finally {
      setBusy(false);
    }
  }

  function onRemove(id: string) {
    if (!confirm(t("keys.deleteConfirm"))) return;
    persistKeys(keys.filter((k) => k.id !== id));
  }

  function onToggle(id: string) {
    persistKeys(
      keys.map((k) => (k.id === id ? { ...k, enabled: !k.enabled } : k)),
    );
  }

  function onRename(id: string, label: string) {
    setKeys((prev) => prev.map((k) => (k.id === id ? { ...k, label } : k)));
  }

  function onWeightChange(id: string, weight: number) {
    const value = Math.max(1, Math.min(20, Number.isFinite(weight) ? weight : 1));
    setKeys((prev) => prev.map((k) => (k.id === id ? { ...k, weight: value } : k)));
  }

  function onRenameCommit() {
    persistKeys(keys.map((k) => ({ ...k, weight: k.weight || 1 })));
  }

  async function onSetPrimary(id: string) {
    try {
      await invoke("set_primary_key_id", { id });
      setProxyInfo((p) => (p ? { ...p, primary_key_id: id } : p));
      flash(t("keys.setPrimaryDone"));
    } catch (e) {
      setError(humanizeError(String(e)));
    }
  }

  async function onTogglePool() {
    if (!proxyInfo) return;
    const next = !proxyInfo.pool_enabled;
    try {
      const info = await invoke<ProxyInfo>("set_pool_enabled", {
        enabled: next,
      });
      setProxyInfo(info);
      flash(next ? t("keys.poolEnabled") : t("keys.poolDisabled"));
    } catch (e) {
      setError(humanizeError(String(e)));
    }
  }

  async function onRegenToken() {
    if (!confirm(t("keys.regenConfirm")))
      return;
    try {
      const tok = await invoke<string>("regenerate_token");
      setProxyInfo((p) => (p ? { ...p, token: tok } : p));
      flash(t("keys.tokenRegenerated"));
    } catch (e) {
      setError(humanizeError(String(e)));
    }
  }

  async function copy(text: string, label: string) {
    try {
      await navigator.clipboard.writeText(text);
      flash(t("keys.copied", { label }));
    } catch {
      flash(t("keys.copyFailed"));
    }
  }

  return (
    <div className="panel view-enter">
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
            <circle cx="8" cy="14" r="4" />
            <path d="M11 14 L20 14 L20 17 M17 14 L17 17" />
          </svg>
        </div>
        <div style={{ flex: 1 }}>
          <h2>
            {t("keys.title")}
            {proxyInfo?.pool_enabled ? (
              <span className="pill ok">{t("keys.poolOn")}</span>
            ) : (
              <span className="pill">{t("keys.directMode")}</span>
            )}
          </h2>
          <div className="sub">
            {t("keys.desc")}
          </div>
        </div>
      </header>

      <div className="panel-divider" />

      <div className="group">
        {/* pool toggle */}
        <div className="switch-row">
          <div>
            <div className="switch-title">{t("keys.enablePool")}</div>
            <div className="hint">
              {t("keys.enablePoolHint")}
            </div>
          </div>
          <button
            className={`switch ${proxyInfo?.pool_enabled ? "on" : ""}`}
            onClick={onTogglePool}
            role="switch"
            aria-checked={proxyInfo?.pool_enabled ?? false}
          >
            <span className="switch-knob" />
          </button>
        </div>

        {/* fail threshold */}
        {proxyInfo?.pool_enabled && (
          <div className="field">
            <label>{t("keys.threshold")}</label>
            <div className="control">
              <div className="input-row">
                <input
                  className="input"
                  type="number"
                  min={1}
                  max={20}
                  value={proxyInfo.fail_threshold}
                  onChange={(e) => {
                    const v = parseInt(e.target.value) || 1;
                    setProxyInfo((p) =>
                      p ? { ...p, fail_threshold: v } : p,
                    );
                  }}
                  onBlur={async (e) => {
                    const v = Math.max(1, parseInt(e.target.value) || 1);
                    try {
                      await invoke("set_fail_threshold", { threshold: v });
                      flash(t("keys.thresholdSet", { v }));
                    } catch (err) {
                      setError(humanizeError(String(err)));
                    }
                  }}
                  style={{ width: 80 }}
                />
                <span className="hint" style={{ paddingTop: 6 }}>
                  {t("keys.thresholdHint")}
                </span>
              </div>
            </div>
          </div>
        )}

        {/* upstream/API address is configured on the Tools page */}

        {/* local endpoint + token (only when pool enabled) */}
        {proxyInfo?.pool_enabled && (
          <>
            <div className="field">
              <label>{t("keys.localEndpoint")}</label>
              <div className="control">
                <div className="endpoint-card">
                  <div className="endpoint-line">
                    <span className="endpoint-label">{t("keys.localHost")}</span>
                    <code>{proxyInfo.local_base}</code>
                    <button
                      className="btn small ghost"
                      onClick={() => copy(proxyInfo.local_base, t("keys.localEndpoint"))}
                    >
                      {t("keys.copy")}
                    </button>
                  </div>
                </div>
              </div>
            </div>

            <div className="field">
              <label>{t("keys.localToken")}</label>
              <div className="control">
                <div className="input-row">
                  <input
                    className="input mono"
                    type={showToken ? "text" : "password"}
                    value={proxyInfo.token}
                    readOnly
                  />
                  <button
                    className="btn"
                    onClick={() => setShowToken((v) => !v)}
                  >
                    {showToken ? t("keys.hide") : t("keys.show")}
                  </button>
                  <button
                    className="btn"
                    onClick={() => copy(proxyInfo.token, "Token")}
                  >
                    {t("keys.copy")}
                  </button>
                  <button className="btn danger" onClick={onRegenToken}>
                    {t("keys.regenerate")}
                  </button>
                </div>
                <div className="hint">
                  {t("keys.tokenHint")}
                </div>
              </div>
            </div>
          </>
        )}

        {/* key list */}
        <div className="field">
          <label>{t("keys.list")}</label>
          <div className="control">
            {keys.length === 0 ? (
              <div className="hint">
                {t("keys.empty")}
              </div>
            ) : (
              <table className="keys-table">
                <thead>
                  <tr>
                    <th style={{ width: 78 }}>{t("keys.status")}</th>
                    <th>{t("keys.name")}</th>
                    <th>{t("keys.secret")}</th>
                    <th style={{ width: 80 }}>{t("keys.weight")}</th>
                    <th style={{ width: 180 }}>{t("keys.stats")}</th>
                    <th style={{ width: 76 }}>{t("keys.recent")}</th>
                    <th style={{ width: 150, textAlign: "right" }}>{t("keys.actions")}</th>
                  </tr>
                </thead>
                <tbody>
                  {keys.map((k) => {
                    const s = stats[k.id];
                    const cooling = s?.cooling_until
                      ? Math.max(
                          0,
                          s.cooling_until - Math.floor(Date.now() / 1000),
                        )
                      : 0;
                    const isPrimary = proxyInfo?.primary_key_id === k.id;
                    return (
                      <tr key={k.id}>
                        <td>
                          {k.enabled ? (
                            cooling > 0 ? (
                              <span className="pill warn">{t("keys.cooling", { s: cooling })}</span>
                            ) : (
                              <span className="pill ok">{t("keys.enabled")}</span>
                            )
                          ) : (
                            <span className="pill">{t("keys.disabled")}</span>
                          )}
                        </td>
                        <td>
                          <input
                            className="input"
                            style={{ padding: "4px 8px" }}
                            value={k.label}
                            onChange={(e) => onRename(k.id, e.target.value)}
                            onBlur={onRenameCommit}
                          />
                          {isPrimary && (
                            <div
                              style={{
                                fontSize: 10.5,
                                color: "var(--accent)",
                                marginTop: 2,
                              }}
                            >
                              {t("keys.primary")}
                            </div>
                          )}
                        </td>
                        <td className="mono">...{k.secret.slice(-6)}</td>
                        <td>
                          <input
                            className="input"
                            type="number"
                            min={1}
                            max={20}
                            value={k.weight || 1}
                            onChange={(e) => onWeightChange(k.id, parseInt(e.target.value) || 1)}
                            onBlur={onRenameCommit}
                            style={{ padding: "4px 8px", width: 64 }}
                            title={t("keys.weightHint")}
                          />
                        </td>
                        <td>
                          <span style={{ color: "var(--success)" }}>
                            +{s?.success ?? 0}
                          </span>
                          &nbsp;/&nbsp;
                          <span style={{ color: "var(--danger)" }}>
                            -{s?.failure ?? 0}
                          </span>
                          <div className="hint" title={t("keys.costHint")}>
                            {t("keys.tokensShort", {
                              count:
                                (s?.input_tokens ?? 0) +
                                (s?.output_tokens ?? 0) +
                                (s?.cache_read_tokens ?? 0) +
                                (s?.cache_write_tokens ?? 0),
                            })}
                            {" · $"}
                            {(s?.estimated_cost ?? 0).toFixed(4)}
                          </div>
                        </td>
                        <td>
                          {s?.last_status ? (
                            <span
                              className="mono"
                              style={{
                                color:
                                  s.last_status >= 400
                                    ? "var(--danger)"
                                    : "var(--success)",
                              }}
                            >
                              {s.last_status}
                            </span>
                          ) : (
                            <span
                              className="mono"
                              style={{ color: "var(--text-soft)" }}
                            >
                              —
                            </span>
                          )}
                        </td>
                        <td>
                          <div className="row-actions">
                            {!isPrimary && (
                              <button
                                className="btn small"
                                onClick={() => onSetPrimary(k.id)}
                                title={t("keys.primary")}
                              >
                                {t("keys.setPrimary")}
                              </button>
                            )}
                            <button
                              className="btn small"
                              onClick={() => onToggle(k.id)}
                            >
                              {k.enabled ? t("keys.disable") : t("keys.enable")}
                            </button>
                            <button
                              className="btn small danger"
                              onClick={() => onRemove(k.id)}
                            >
                              {t("keys.delete")}
                            </button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}

            <div className="add-key-row">
              <input
                className="input"
                placeholder={t("keys.labelPlaceholder")}
                value={newLabel}
                onChange={(e) => setNewLabel(e.target.value)}
              />
              <input
                className="input"
                type="password"
                placeholder={t("keys.secretPlaceholder")}
                value={newSecret}
                onChange={(e) => setNewSecret(e.target.value)}
              />
              <button
                className="btn primary"
                onClick={onAdd}
                disabled={busy || !newSecret.trim()}
              >
                {t("keys.addKey")}
              </button>
            </div>

            <div className="bulk-import-card">
              <div className="switch-title">{t("keys.urlImport")}</div>
              <div className="hint">{t("keys.urlImportHint")}</div>
              <div className="input-row" style={{ marginTop: 10 }}>
                <button
                  className="btn primary"
                  onClick={onEnableDeepLink}
                  disabled={busy}
                >
                  {t("keys.enableDeepLink")}
                </button>
              </div>
              <div className="hint" style={{ marginTop: 8 }}>
                {t("keys.urlImportExample")}{" "}
                <code>beeapi-switch://import?key=sk-xxx&amp;name=key-1</code>
              </div>
            </div>
            <div className="hint">
              {t("keys.storageHint")}
            </div>
          </div>
        </div>
      </div>

      {notice && (
        <div className="alert ok toast-in">
          <div style={{ flex: 1 }}>{notice}</div>
        </div>
      )}
      {error && (
        <div className="alert err toast-in">
          <div style={{ flex: 1 }}>
            <strong>{t("tools.failed")}</strong>
            <pre>{error}</pre>
          </div>
        </div>
      )}
    </div>
  );
}
