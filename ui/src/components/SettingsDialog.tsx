import React, { useEffect, useState } from "react";
import {
  api,
  backendKind,
  type AppSettingsInfo,
  type ConnectionResult,
  type ModelInfo,
} from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";
import ModelSelect from "./ModelSelect";

type ConnState =
  | { phase: "idle" }
  | { phase: "busy" }
  | { phase: "done"; ok: boolean; text: string };

type ModelState =
  | { phase: "idle" }
  | { phase: "busy" }
  | { phase: "done"; ok: boolean; text: string };

const SettingsDialog: React.FC<{
  onClose: () => void;
  onSettingsChanged: () => void;
}> = ({ onClose, onSettingsChanged }) => {
  const toast = useToast();
  const [settings, setSettings] = useState<AppSettingsInfo | null>(null);
  const [keyInput, setKeyInput] = useState("");
  const [conn, setConn] = useState<ConnState>({ phase: "idle" });
  const [modelTest, setModelTest] = useState<ModelState>({ phase: "idle" });
  const [discovered, setDiscovered] = useState<ModelInfo[] | null>(null);
  const [manualModel, setManualModel] = useState(false);
  const [savingKey, setSavingKey] = useState(false);
  const [theme, setTheme] = useState<"dark" | "light">(
    (document.documentElement.classList.contains("light") ? "light" : "dark") as "dark" | "light",
  );
  const [doctorText, setDoctorText] = useState<string | null>(null);

  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        setSettings(s);
        if (!s.keyStored) {
          // No key yet: nothing else to prep.
        }
      })
      .catch((e) => toast.show(errMsg(e), "bad"));
    api
      .doctor()
      .then((d) =>
        setDoctorText(
          `${d.appVersion} · ${d.runtime} · FFmpeg ${d.ffmpeg.state}${d.ffmpeg.version ? ` ${d.ffmpeg.version}` : ""}${d.ffmpeg.usingBundled ? " (bundled)" : ""}`,
        ),
      )
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveKey = async () => {
    if (!keyInput.trim()) {
      toast.show("Paste your key first (keys are never displayed back).", "bad");
      return;
    }
    setSavingKey(true);
    try {
      await api.setApiKey(keyInput.trim());
      setKeyInput("");
      toast.show("API key saved", "ok");
      const s = await api.getSettings();
      setSettings(s);
      onSettingsChanged();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    } finally {
      setSavingKey(false);
    }
  };

  const clearKey = async () => {
    try {
      await api.clearApiKey();
      toast.show("API key removed", "ok");
      const s = await api.getSettings();
      setSettings(s);
      onSettingsChanged();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const testConnection = async (): Promise<ConnectionResult> => {
    setConn({ phase: "busy" });
    let result: ConnectionResult;
    try {
      result = await api.testConnection();
    } catch (e) {
      result = { ok: false, step: "models", state: errMsg(e) };
    }
    setConn({ phase: "done", ok: result.ok, text: result.state });
    if (result.ok && (result.model_count ?? 0) > 0) {
      // Discovery succeeded — load the list for the selector.
      try {
        const r = await api.listModels(false);
        setDiscovered(r.models);
        setManualModel(false);
      } catch {
        setManualModel(true);
      }
    }
    onSettingsChanged();
    return result;
  };

  const testModel = async () => {
    setModelTest({ phase: "busy" });
    try {
      const r = await api.testModel();
      setModelTest({ phase: "done", ok: r.ok, text: r.state });
    } catch (e) {
      setModelTest({ phase: "done", ok: false, text: errMsg(e) });
    }
  };

  const patchSetting = async (key: string, value: string | boolean) => {
    try {
      await api.setSetting(key, value);
      onSettingsChanged();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const clearCache = async () => {
    try {
      await api.clearCache();
      toast.show(t.settings.cacheCleared, "ok");
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const switchTheme = (next: "dark" | "light") => {
    setTheme(next);
    document.documentElement.classList.toggle("light", next === "light");
    try {
      localStorage.setItem("mycut.theme", next);
    } catch {
      /* private mode */
    }
  };

  const selectedModelGone =
    discovered !== null &&
    !!settings?.model &&
    discovered.length > 0 &&
    !discovered.some((m) => m.id === settings.model);

  return (
    <div className="modal-back" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal wide" role="dialog" aria-label={t.settings.title}>
        <div className="modal-head">
          {t.settings.title}
          <button className="ghost" onClick={onClose} aria-label="Close">✕</button>
        </div>
        <div className="modal-body">
          {/* ---------------- AI PROVIDER ---------------- */}
          <div className="panel-section">
            <h4>{t.settings.sectionProvider}</h4>
            <div className="stack">
              <label>
                {t.settings.apiKey}
                <input
                  type="password"
                  value={keyInput}
                  placeholder={settings?.keyStored ? "•••••••••••• (saved)" : "nvapi-…"}
                  onChange={(e) => setKeyInput(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && saveKey()}
                  autoComplete="off"
                />
                <span className="hint">{t.settings.apiKeyHint}</span>
              </label>
              <div className="row">
                <button className="primary" onClick={saveKey} disabled={savingKey}>
                  {t.settings.saveKey}
                </button>
                {settings?.keyStored && (
                  <button className="danger" onClick={clearKey}>{t.settings.clearKey}</button>
                )}
                <span className={`tiny${settings?.keyStored ? "" : ""}`} style={{ color: settings?.keyStored ? "var(--ok)" : "var(--text-2)" }}>
                  {settings?.keyStored
                    ? `✓ ${t.settings.keyStored} (${settings.keyBackend})`
                    : t.settings.keyMissing}
                </span>
              </div>
              <label>
                {t.settings.baseUrl}
                <input
                  type="text"
                  value={settings?.baseUrl ?? ""}
                  onChange={(e) => setSettings((s) => (s ? { ...s, baseUrl: e.target.value } : s))}
                  onBlur={(e) => patchSetting("base_url", e.target.value)}
                />
              </label>
              <div className="row">
                <button onClick={testConnection} disabled={conn.phase === "busy"}>
                  {conn.phase === "busy" ? t.settings.testing : t.settings.testConnection}
                </button>
              </div>
              {conn.phase !== "idle" && (
                <div className={`status-line ${conn.phase === "busy" ? "busy" : conn.ok ? "ok" : "bad"}`}>
                  {conn.phase === "busy" ? t.settings.testing : conn.text}
                </div>
              )}
            </div>
          </div>

          {/* ---------------- MODEL ---------------- */}
          <div className="panel-section">
            <h4>{t.settings.sectionModel}</h4>
            {manualModel ? (
              <div className="stack">
                <div className="status-line">{t.settings.discoveryUnavailable}</div>
                <label>
                  {t.settings.manualModelId}
                  <input
                    type="text"
                    value={settings?.model ?? ""}
                    onChange={(e) => setSettings((s) => (s ? { ...s, model: e.target.value } : s))}
                    onBlur={(e) => patchSetting("model", e.target.value)}
                  />
                </label>
              </div>
            ) : (
              <div className="stack">
                <ModelSelect
                  value={settings?.model ?? ""}
                  onChanged={(m) => {
                    setSettings((s) => (s ? { ...s, model: m } : s));
                    void patchSetting("model", m);
                  }}
                  onListLoaded={(models) => {
                    setDiscovered(models);
                    setManualModel(models.length === 0);
                  }}
                />
                {selectedModelGone && (
                  <div className="status-line bad">{t.settings.modelGone}</div>
                )}
              </div>
            )}
            <div className="row" style={{ marginTop: 8 }}>
              <button onClick={testModel} disabled={modelTest.phase === "busy" || !settings?.model}>
                {modelTest.phase === "busy" ? t.settings.testingModel : t.settings.testModel}
              </button>
              {modelTest.phase === "done" && (
                <span className={`tiny${modelTest.ok ? "" : ""}`} style={{ color: modelTest.ok ? "var(--ok)" : "var(--bad)" }}>
                  {modelTest.ok ? "✓ " : "✗ "}
                  {modelTest.text}
                </span>
              )}
            </div>
          </div>

          {/* ---------------- GENERAL ---------------- */}
          <div className="panel-section">
            <h4>{t.settings.sectionGeneral}</h4>
            <div className="grid2">
              <label>
                {t.settings.theme}
                <select value={theme} onChange={(e) => switchTheme(e.target.value as "dark" | "light")}>
                  <option value="dark">{t.settings.themeDark}</option>
                  <option value="light">{t.settings.themeLight}</option>
                </select>
              </label>
              <label>
                {t.settings.profile}
                <select
                  value={settings?.profile ?? "low"}
                  onChange={(e) => {
                    setSettings((s) => (s ? { ...s, profile: e.target.value } : s));
                    void patchSetting("profile", e.target.value);
                  }}
                >
                  <option value="low">Low (8 GB RAM friendly)</option>
                  <option value="balanced">Balanced</option>
                </select>
              </label>
            </div>
            <label className="row" style={{ gap: 8, marginTop: 8 }}>
              <input
                type="checkbox"
                checked={settings?.framesEnabled ?? false}
                onChange={(e) => {
                  setSettings((s) => (s ? { ...s, framesEnabled: e.target.checked } : s));
                  void patchSetting("frames_enabled", e.target.checked);
                }}
              />
              {t.settings.frameSharing} — {t.settings.frameSharingHint}
            </label>
            <div className="row" style={{ marginTop: 10 }}>
              <button className="small" onClick={clearCache}>{t.settings.clearCache}</button>
            </div>
          </div>

          {/* ---------------- DIAGNOSTICS ---------------- */}
          <div className="panel-section">
            <h4>{t.settings.diagnostics}</h4>
            <div className="status-line">{doctorText ?? "…"}</div>
            {backendKind() === "none" && (
              <div className="status-line bad">{t.errors.backendOffline}</div>
            )}
          </div>
        </div>
        <div className="modal-foot">
          <button className="primary" onClick={onClose}>Done</button>
        </div>
      </div>
    </div>
  );
};

export default SettingsDialog;
