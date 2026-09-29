import { useEffect, useState } from "react";
import { api } from "../lib/bridge";
import { strings as t } from "../i18n/en";

const SettingsPanel: React.FC = () => {
  const [key, setKey] = useState("");
  const [settings, setSettings] = useState<{ baseUrl: string; model: string; framesEnabled: boolean; profile: string } | null>(null);
  const [state, setState] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => undefined);
  }, []);

  const test = async () => {
    await api.setApiKey(key).catch(() => undefined);
    try {
      const r = await api.testConnection();
      setState({ ok: r.ok, text: r.ok ? t.settings.connected : r.state });
    } catch (e) {
      setState({ ok: false, text: String(e) });
    }
  };

  return (
    <div className="form">
      <label>
        {t.settings.apiKey}
        <input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder="nvapi-…" />
        <span>{t.settings.apiKeyHint}</span>
      </label>
      <button className="btn" onClick={test}>
        {t.settings.testConnection}
      </button>
      {state && <div className={`badge ${state.ok ? "state-ok" : "state-bad"}`}>{state.text}</div>}
      {settings && (
        <>
          <label>
            {t.settings.baseUrl}
            <input type="text" value={settings.baseUrl} onChange={(e) => setSettings({ ...settings, baseUrl: e.target.value })} onBlur={() => api.setSetting("base_url", settings.baseUrl)} />
          </label>
          <label>
            {t.settings.model}
            <input type="text" value={settings.model} onChange={(e) => setSettings({ ...settings, model: e.target.value })} onBlur={() => api.setSetting("model", settings.model)} />
          </label>
          <label>
            <input type="checkbox" checked={settings.framesEnabled} onChange={(e) => { setSettings({ ...settings, framesEnabled: e.target.checked }); api.setSetting("frames_enabled", e.target.checked); }} />
            {t.settings.frameSharing} — {t.settings.frameSharingHint}
          </label>
        </>
      )}
    </div>
  );
};

export default SettingsPanel;
