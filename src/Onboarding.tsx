import { t } from "./i18n";

interface Props {
  onDone: () => void;
}

export default function Onboarding({ onDone }: Props) {
  return (
    <div className="onboarding-overlay">
      <div className="onboarding-card">
        <div className="onboarding-logo">🐝</div>
        <h2>{t("onboarding.title")}</h2>

        <div className="onboarding-steps">
          <div className="onboarding-step">
            <div className="step-num">1</div>
            <div>
              <div className="step-title">{t("onboarding.step1Title")}</div>
              <div className="step-desc">{t("onboarding.step1Desc")}</div>
            </div>
          </div>
          <div className="onboarding-step">
            <div className="step-num">2</div>
            <div>
              <div className="step-title">{t("onboarding.step2Title")}</div>
              <div className="step-desc">{t("onboarding.step2Desc")}</div>
            </div>
          </div>
          <div className="onboarding-step">
            <div className="step-num">3</div>
            <div>
              <div className="step-title">{t("onboarding.step3Title")}</div>
              <div className="step-desc">{t("onboarding.step3Desc")}</div>
            </div>
          </div>
        </div>

        <div className="onboarding-actions">
          <button className="btn primary" onClick={onDone}>
            {t("onboarding.start")}
          </button>
          <button className="btn ghost" onClick={onDone}>
            {t("onboarding.skip")}
          </button>
        </div>
      </div>
    </div>
  );
}
