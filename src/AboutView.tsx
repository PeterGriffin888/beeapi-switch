import { useState } from "react";
import { check } from "@tauri-apps/plugin-updater";
import { openUrl } from "@tauri-apps/plugin-opener";
import { t } from "./i18n";

const APP_VERSION = __APP_VERSION__;

export default function AboutView() {
  const [checking, setChecking] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [progress, setProgress] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [upToDate, setUpToDate] = useState(false);

  async function onCheckUpdate() {
    setChecking(true);
    setError(null);
    setUpToDate(false);
    setProgress(null);
    try {
      const update = await check();
      if (update) {
        setProgress(t("about.downloading", { version: update.version }));
        setUpdating(true);
        let downloaded = 0;
        let total = 0;
        await update.downloadAndInstall((event) => {
          if (event.event === "Started") {
            total = (event.data as any).contentLength || 0;
          } else if (event.event === "Progress") {
            downloaded += (event.data as any).chunkLength || 0;
            if (total > 0) {
              const pct = Math.round((downloaded / total) * 100);
              setProgress(t("about.downloadProgress", { pct }));
            }
          } else if (event.event === "Finished") {
            setProgress(t("about.installing"));
          }
        });
        // After install, Tauri will restart the app
      } else {
        setUpToDate(true);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
      setUpdating(false);
    }
  }

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
            <circle cx="12" cy="12" r="10" />
            <line x1="12" y1="16" x2="12" y2="12" />
            <line x1="12" y1="8" x2="12.01" y2="8" />
          </svg>
        </div>
        <div style={{ flex: 1 }}>
          <h2>{t("about.title")}</h2>
          <div className="sub">{t("about.desc")}</div>
        </div>
      </header>

      <div className="panel-divider" />

      <div className="about-info">
        <div className="about-row">
          <span className="about-label">{t("about.appName")}</span>
          <span className="about-value">BeeAPI Switch</span>
        </div>
        <div className="about-row">
          <span className="about-label">{t("about.version")}</span>
          <span className="about-value">v{APP_VERSION}</span>
        </div>
        <div className="about-row">
          <span className="about-label">{t("about.author")}</span>
          <span className="about-value">BeeAPI</span>
        </div>
        <div className="about-row">
          <span className="about-label">{t("about.project")}</span>
          <button
            className="btn ghost small"
            onClick={() => openUrl("https://github.com/PeterGriffin888/beeapi-switch")}
          >
            github.com/PeterGriffin888/beeapi-switch
          </button>
        </div>
        <div className="about-row">
          <span className="about-label">{t("about.reference")}</span>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <button
              className="btn ghost small"
              onClick={() => openUrl("https://github.com/farion1231/cc-switch")}
            >
              {t("about.referenceCcSwitch")}
            </button>
            <button
              className="btn ghost small"
              onClick={() => openUrl("https://github.com/jlcodes99/cockpit-tools")}
            >
              {t("about.referenceCockpitTools")}
            </button>
            <button
              className="btn ghost small"
              onClick={() => openUrl("https://github.com/BigPizzaV3/CodexPlusPlus")}
            >
              {t("about.referenceCodexPlusPlus")}
            </button>
          </div>
        </div>
      </div>

      <div className="panel-divider" />

      <div className="group">
        <div className="switch-row" style={{ flexDirection: "column", alignItems: "flex-start", gap: 12 }}>
          <div>
            <div className="switch-title">{t("about.updateTitle")}</div>
            <div className="hint">{t("about.updateHint")}</div>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <button
              className="btn primary"
              onClick={onCheckUpdate}
              disabled={checking || updating}
            >
              {checking ? (
                <>
                  <span className="spinner" /> {t("about.checking")}
                </>
              ) : updating ? (
                <>
                  <span className="spinner" /> {t("about.updating")}
                </>
              ) : (
                t("about.checkUpdate")
              )}
            </button>
            {upToDate && (
              <span style={{ color: "var(--success)", fontSize: 12 }}>
                ✓ {t("about.upToDate")}
              </span>
            )}
          </div>
          {progress && (
            <div className="hint" style={{ color: "var(--text)" }}>{progress}</div>
          )}
          {error && (
            <div className="alert err toast-in" style={{ marginTop: 8 }}>
              <div style={{ flex: 1 }}>
                <pre>{error}</pre>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
