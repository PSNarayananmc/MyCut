import React, { useCallback, useEffect, useRef, useState } from "react";
import { api, assetUrl, backendAvailable, backendKind, type ProjectSnapshot } from "./lib/bridge";
import { strings as t } from "./i18n/en";
import MediaBin from "./components/MediaBin";
import TimelineView from "./components/TimelineView";
import ChatPanel from "./components/ChatPanel";
import ExportPanel from "./components/ExportPanel";
import SettingsPanel from "./components/SettingsPanel";

const App: React.FC = () => {
  const [snapshot, setSnapshot] = useState<ProjectSnapshot | null>(null);
  const [tab, setTab] = useState<"export" | "settings">("export");
  const [previewSrc, setPreviewSrc] = useState<string | null>(null);
  const [offline, setOffline] = useState(!backendAvailable());
  const videoRef = useRef<HTMLVideoElement>(null);

  const refresh = useCallback(() => {
    api
      .projectSnapshot()
      .then(setSnapshot)
      .catch(() => setOffline(true));
  }, []);

  useEffect(() => {
    if (backendAvailable()) refresh();
  }, [refresh]);

  const onImport = useCallback(() => {
    if (backendKind() === "server") {
      // The browser cannot hand the server absolute paths; the local web
      // runtime imports by explicit path (Tauri build has the native dialog).
      const raw = window.prompt(
        "Import media — absolute path(s), comma-separated\n(e.g. /home/you/Videos/clip1.mp4, /home/you/Videos/clip2.mp4)",
        "",
      );
      if (!raw || !raw.trim()) return;
      const paths = raw.split(",").map((s) => s.trim()).filter(Boolean);
      api.importMedia(paths).then(() => refresh()).catch((e) => alert(String(e)));
      return;
    }
    // The Rust side opens the native file dialog (paths stay in Rust).
    api
      .importMedia([])
      .then(() => refresh())
      .catch((e) => alert(String(e)));
  }, [refresh]);

  const onPlayProxy = useCallback(async () => {
    if (!snapshot?.sources[0]) return;
    try {
      const { path } = await api.proxyPath(snapshot.sources[0].id);
      setPreviewSrc(assetUrl(path));
      requestAnimationFrame(() => videoRef.current?.play().catch(() => undefined));
    } catch (e) {
      alert(String(e));
    }
  }, [snapshot]);

  return (
    <div className="app">
      <header className="topbar">
        <div className="logo">
          My<span>Cut</span>
        </div>
        <button onClick={onImport} disabled={offline}>
          {t.media.import}
        </button>
        <button onClick={onPlayProxy} disabled={offline || !snapshot?.sources.length}>
          ▶ {t.panels.preview}
        </button>
        <div className="spacer" />
        <button className="btn-ghost" onClick={() => api.undo().then(refresh)} disabled={offline}>
          ↶ {t.timeline.undo.split(" ")[0]}
        </button>
        <button className="btn-ghost" onClick={() => api.redo().then(refresh)} disabled={offline}>
          ↷ {t.timeline.redo.split(" ")[0]}
        </button>
        <button className="btn-ghost" onClick={() => api.save()} disabled={offline}>
          Save
        </button>
        {offline && <span className="badge net">backend offline (UI dev mode)</span>}
        {!offline && backendKind() === "server" && (
          <span className="badge net" title="Editing runs in this local process; the UI is served to your browser.">local runtime</span>
        )}
      </header>

      <div className="main">
        <aside className="left">
          <div className="panel-title">{t.panels.media}</div>
          <MediaBin snapshot={snapshot} onRefresh={refresh} />
          <div className="tabs">
            <button className={tab === "export" ? "active" : ""} onClick={() => setTab("export")}>
              {t.panels.export}
            </button>
            <button className={tab === "settings" ? "active" : ""} onClick={() => setTab("settings")}>
              {t.panels.settings}
            </button>
          </div>
          {tab === "export" ? <ExportPanel offline={offline} /> : <SettingsPanel />}
        </aside>

        <section className="center">
          <div className="preview">
            {previewSrc ? (
              <video ref={videoRef} src={previewSrc} controls />
            ) : (
              <div className="notice">{t.panels.preview} — proxies render on demand</div>
            )}
          </div>
          <div className="panel-title">{t.panels.timeline}</div>
          <TimelineView snapshot={snapshot} onRefresh={refresh} />
          <div className="known-limitations">
            <strong>{t.limitations.title}:</strong> preview renders are on-demand proxy clips (not a
            real-time compositor); multi-track video compositing and keyframe editing are planned.
          </div>
        </section>

        <aside className="right">
          <div className="panel-title">{t.panels.chat}</div>
          <ChatPanel offline={offline} onRefresh={refresh} />
        </aside>
      </div>
    </div>
  );
};

export default App;
