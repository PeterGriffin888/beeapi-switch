import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import {
  KeysTabIcon,
  SessionsTabIcon,
  PlaygroundTabIcon,
  ThemeIcon,
  UsageTabIcon,
  ToolsTabIcon,
  AboutTabIcon,
} from "./icons";
import { getLocale, setLocale, t as t_fn, Locale } from "./i18n";

const IS_MACOS = navigator.userAgent.includes("Macintosh") || navigator.userAgent.includes("Mac OS");
const IS_WIN = !IS_MACOS;

export type TabId = "tools" | "keys" | "usage" | "sessions" | "playground" | "about";

interface Props {
  tab: TabId;
  onTab: (t: TabId) => void;
  theme: "light" | "dark";
  onToggleTheme: () => void;
}

const TABS: { id: TabId; label: string; icon: () => JSX.Element }[] = [
  { id: "tools", label: "tab.tools", icon: () => <ToolsTabIcon /> },
  { id: "keys", label: "tab.keys", icon: () => <KeysTabIcon /> },
  { id: "usage", label: "tab.usage", icon: () => <UsageTabIcon /> },
  { id: "sessions", label: "tab.sessions", icon: () => <SessionsTabIcon /> },
  { id: "playground", label: "tab.playground", icon: () => <PlaygroundTabIcon /> },
  { id: "about", label: "tab.about", icon: () => <AboutTabIcon /> },
];

export default function Titlebar({ tab, onTab, theme, onToggleTheme }: Props) {
  const [proxyOk, setProxyOk] = useState<boolean | null>(null);
  const [poolEnabled, setPoolEnabled] = useState(false);
  const [showCloseDialog, setShowCloseDialog] = useState(false);

  useEffect(() => {
    checkHealth();
    const interval = setInterval(checkHealth, 5000);
    return () => clearInterval(interval);
  }, []);

  async function checkHealth() {
    try {
      const ok = await invoke<boolean>("proxy_health");
      setProxyOk(ok);
      const info = await invoke<{ pool_enabled: boolean }>("proxy_info");
      setPoolEnabled(info.pool_enabled);
    } catch {
      setProxyOk(false);
    }
  }

  function onCloseClick() {
    setShowCloseDialog(true);
  }

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("beeapi-close-requested", () => setShowCloseDialog(true)).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  async function onMinimizeToTray() {
    setShowCloseDialog(false);
    await invoke("window_close"); // This now hides the window
  }

  async function onQuit() {
    setShowCloseDialog(false);
    await invoke("window_quit");
  }

  // Listen for Escape to close the dialog
  useEffect(() => {
    if (!showCloseDialog) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setShowCloseDialog(false);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [showCloseDialog]);

  return (
    <div className={`titlebar ${IS_WIN ? "titlebar-win" : ""}`}>
      <div className="title">
        <div className="mark">B</div>
        <span>BeeAPI Switch</span>
        {poolEnabled && (
          <span
            className={`proxy-dot ${proxyOk === true ? "ok" : proxyOk === false ? "err" : ""}`}
            title={
              proxyOk === true
                ? t_fn("proxy.ok")
                : proxyOk === false
                  ? t_fn("proxy.err")
                  : t_fn("proxy.checking")
            }
          />
        )}
      </div>

      <nav className="tab-bar" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`tab ${tab === t.id ? "active" : ""}`}
            role="tab"
            aria-selected={tab === t.id}
            onClick={() => onTab(t.id)}
          >
            <span className="tab-icon">{t.icon()}</span>
            <span>{t_fn(t.label)}</span>
          </button>
        ))}
      </nav>

      <div className="spacer" />

      <button
        className="lang-switch"
        onClick={() => {
          const next: Locale = getLocale() === "zh" ? "en" : "zh";
          setLocale(next);
          window.location.reload();
        }}
        title="Switch language"
      >
        {getLocale() === "zh" ? "EN" : "中"}
      </button>

      <button
        className="theme-switch"
        onClick={onToggleTheme}
        title={theme === "light" ? t_fn("titlebar.switchToDark") : t_fn("titlebar.switchToLight")}
      >
        <ThemeIcon theme={theme} />
        <span>{theme === "light" ? t_fn("theme.light") : t_fn("theme.dark")}</span>
      </button>

      {!IS_MACOS && (
        <div className="win-controls">
          <button
            className="win-btn"
            onClick={() => invoke("window_minimize")}
            title={t_fn("titlebar.minimize")}
          >
            <svg viewBox="0 0 12 12" aria-hidden>
              <rect x="2" y="5.5" width="8" height="1" fill="currentColor" />
            </svg>
          </button>
          <button
            className="win-btn"
            onClick={() => invoke("window_toggle_maximize")}
            title={t_fn("titlebar.maximizeRestore")}
          >
            <svg viewBox="0 0 12 12" aria-hidden>
              <rect
                x="2"
                y="2"
                width="8"
                height="8"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
              />
            </svg>
          </button>
          <button
            className="win-btn close"
            onClick={onCloseClick}
            title={t_fn("titlebar.close")}
          >
            <svg viewBox="0 0 12 12" aria-hidden>
              <path
                d="M2 2 L10 10 M10 2 L2 10"
                stroke="currentColor"
                strokeWidth="1.2"
                fill="none"
              />
            </svg>
          </button>
        </div>
      )}

      {showCloseDialog && (
        <div className="confirm-overlay" onClick={() => setShowCloseDialog(false)}>
          <div className="confirm-card" onClick={(e) => e.stopPropagation()}>
            <div className="confirm-icon" style={{ borderColor: "var(--border-strong)", color: "var(--text-dim)", background: "var(--surface-2)" }}>
              <svg
                width="22"
                height="22"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.8"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <path d="M18 6 L6 18 M6 6 L18 18" />
              </svg>
            </div>
            <h3 className="confirm-title">{t_fn("close.title")}</h3>
            <p className="confirm-message">{t_fn("close.message")}</p>
            <div className="confirm-actions" style={{ flexDirection: "column", gap: 8 }}>
              <button className="btn primary" style={{ width: "100%" }} onClick={onMinimizeToTray}>
                {t_fn("close.minimize")}
              </button>
              <button className="btn danger" style={{ width: "100%" }} onClick={onQuit}>
                {t_fn("close.quit")}
              </button>
              <button className="btn ghost" style={{ width: "100%" }} onClick={() => setShowCloseDialog(false)}>
                {t_fn("dialog.cancel")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
