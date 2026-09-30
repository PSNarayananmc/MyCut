import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  backendAvailable,
  type ProjectSnapshot,
  type SourceInfo,
} from "./lib/bridge";
import { strings as t } from "./i18n/en";
import { ToastProvider, useToast, errMsg } from "./components/Toast";
import TopBar, { type ConnState } from "./components/TopBar";
import MediaPanel from "./components/MediaPanel";
import EffectsPanel from "./components/EffectsPanel";
import TransitionsPanel from "./components/TransitionsPanel";
import TextPanel from "./components/TextPanel";
import CaptionsPanel from "./components/CaptionsPanel";
import AudioPanel from "./components/AudioPanel";
import FiltersPanel from "./components/FiltersPanel";
import AIPanel from "./components/AIPanel";
import Preview from "./components/Preview";
import Timeline from "./components/Timeline";
import Inspector from "./components/Inspector";
import ExportDialog from "./components/ExportDialog";
import SettingsDialog from "./components/SettingsDialog";

type RailTab = "media" | "audio" | "text" | "captions" | "effects" | "transitions" | "filters" | "ai";

const RAIL_TABS: { id: RailTab; label: string }[] = [
  { id: "media", label: t.panels.media },
  { id: "audio", label: t.panels.audio },
  { id: "text", label: t.panels.text },
  { id: "captions", label: t.panels.captions },
  { id: "effects", label: t.panels.effects },
  { id: "transitions", label: t.panels.transitions },
  { id: "filters", label: t.panels.filters },
  { id: "ai", label: `${t.panels.ai} ✦` },
];

const Workspace: React.FC = () => {
  const toast = useToast();
  const [snapshot, setSnapshot] = useState<ProjectSnapshot | null>(null);
  const [offline, setOffline] = useState(!backendAvailable());
  const [tab, setTab] = useState<RailTab>("media");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [playheadMs, setPlayheadMs] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [previewSource, setPreviewSource] = useState<SourceInfo | null>(null);
  const [draggingSourceId, setDraggingSourceId] = useState<string | null>(null);
  const [showExport, setShowExport] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [conn] = useState<ConnState>("unknown");
  const [connText, setConnText] = useState("");
  const [modelName, setModelName] = useState<string | null>(null);
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;

  const refresh = useCallback(() => {
    api
      .projectSnapshot()
      .then((s) => {
        setSnapshot(s);
        setOffline(false);
      })
      .catch(() => setOffline(true));
  }, []);

  // Initial load + model/settings probe.
  useEffect(() => {
    if (!backendAvailable()) return;
    refresh();
    api
      .getSettings()
      .then((s) => {
        setModelName(s.model);
        setConnText(s.keyStored ? "" : t.app.notConnected);
      })
      .catch(() => undefined);
  }, [refresh]);

  // Theme restore.
  useEffect(() => {
    try {
      if (localStorage.getItem("mycut.theme") === "light") {
        document.documentElement.classList.add("light");
      }
    } catch {
      /* private mode */
    }
  }, []);

  const onRename = useCallback((name: string) => {
    setSnapshot((s) => (s ? { ...s, name } : s));
  }, []);

  // Keyboard shortcuts (spec §17).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      const typing =
        target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable;
      const inModal = !!document.querySelector(".modal-back");
      if (typing || inModal) return;
      const mod = e.ctrlKey || e.metaKey;
      if (e.code === "Space") {
        e.preventDefault();
        const v = document.querySelector<HTMLVideoElement>(".preview video");
        if (v) {
          if (v.paused) v.play().catch(() => undefined);
          else v.pause();
        }
      } else if (mod && e.key.toLowerCase() === "z" && !e.shiftKey) {
        e.preventDefault();
        api.undo().then(refresh).catch((er) => toast.show(errMsg(er), "bad"));
      } else if (mod && (e.key.toLowerCase() === "z" && e.shiftKey)) {
        e.preventDefault();
        api.redo().then(refresh).catch((er) => toast.show(errMsg(er), "bad"));
      } else if (mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        api.save().then(() => toast.show(t.app.savedOk, "ok")).catch((er) => toast.show(errMsg(er), "bad"));
      } else if (mod && e.key.toLowerCase() === "e") {
        e.preventDefault();
        setShowExport(true);
      } else if (mod && e.key.toLowerCase() === "d") {
        e.preventDefault();
        if (selectedId) {
          api.duplicateItem(selectedId).then(refresh).catch((er) => toast.show(errMsg(er), "bad"));
        }
      } else if (e.key === "Delete" || e.key === "Backspace") {
        if (selectedId) {
          e.preventDefault();
          api.deleteItem(selectedId).then(refresh).catch((er) => toast.show(errMsg(er), "bad"));
        }
      } else if (e.key.toLowerCase() === "s" && !mod) {
        // Split selected clip at playhead.
        if (selectedId) {
          api
            .splitClip(selectedId, playheadMs)
            .then(refresh)
            .catch((er) => toast.show(errMsg(er), "bad"));
        }
      } else if (e.key === "+" || e.key === "=") {
        setZoom((z) => Math.min(5, z * 1.25));
      } else if (e.key === "-") {
        setZoom((z) => Math.max(0.2, z / 1.25));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [refresh, toast, selectedId, playheadMs]);

  const onSeek = useCallback((ms: number) => {
    setPlayheadMs(Math.max(0, ms));
  }, []);

  const onPreviewSource = useCallback(
    (s: SourceInfo) => {
      setPreviewSource(s);
    },
    [],
  );

  return (
    <div className="app">
      <TopBar
        snapshot={snapshot}
        offline={offline}
        conn={conn}
        connText={connText}
        onRename={onRename}
        onRefresh={refresh}
        onExport={() => setShowExport(true)}
        onSettings={() => setShowSettings(true)}
      />
      <div className="main">
        <aside className="left">
          <div className="rail-tabs" role="tablist">
            {RAIL_TABS.map((rt) => (
              <button
                key={rt.id}
                role="tab"
                aria-selected={tab === rt.id}
                className={tab === rt.id ? "active" : ""}
                onClick={() => setTab(rt.id)}
              >
                {rt.label}
              </button>
            ))}
          </div>
          <div className="rail-body">
            {tab === "media" && (
              <MediaPanel
                snapshot={snapshot}
                onRefresh={refresh}
                onPreviewSource={onPreviewSource}
                onDragStartSource={(s) => setDraggingSourceId(s.id)}
              />
            )}
            {tab === "audio" && <AudioPanel snapshot={snapshot} selectedId={selectedId} onRefresh={refresh} />}
            {tab === "text" && (
              <TextPanel
                snapshot={snapshot}
                selectedId={selectedId}
                onSelect={setSelectedId}
                playheadMs={playheadMs}
                onRefresh={refresh}
              />
            )}
            {tab === "captions" && (
              <CaptionsPanel snapshot={snapshot} playheadMs={playheadMs} onRefresh={refresh} />
            )}
            {tab === "effects" && (
              <EffectsPanel snapshot={snapshot} selectedId={selectedId} onRefresh={refresh} />
            )}
            {tab === "transitions" && <TransitionsPanel snapshot={snapshot} onRefresh={refresh} />}
            {tab === "filters" && <FiltersPanel snapshot={snapshot} onRefresh={refresh} />}
            {tab === "ai" && (
              <AIPanel
                offline={offline}
                modelName={modelName}
                connState={conn}
                onRefresh={refresh}
                onOpenSettings={() => setShowSettings(true)}
              />
            )}
          </div>
        </aside>

        <section className="center">
          <Preview
            snapshot={snapshot}
            offline={offline}
            playheadMs={playheadMs}
            onSeek={onSeek}
            previewSource={previewSource}
          />
          <Timeline
            snapshot={snapshot}
            offline={offline}
            selectedId={selectedId}
            onSelect={setSelectedId}
            onRefresh={refresh}
            onSeek={onSeek}
            onSplitAt={(ms) => {
              if (selectedId) {
                api
                  .splitClip(selectedId, ms)
                  .then(refresh)
                  .catch((e) => toast.show(errMsg(e), "bad"));
              }
            }}
            playheadMs={playheadMs}
            zoom={zoom}
            setZoom={setZoom}
            draggingSourceId={draggingSourceId}
          />
        </section>

        <aside className="right">
          <div className="panel-title" style={{ padding: "10px 12px 0", fontWeight: 700, fontSize: 12, textTransform: "uppercase", letterSpacing: "0.8px", color: "var(--text-2)" }}>
            {t.panels.inspector}
          </div>
          <div className="rail-body">
            <Inspector snapshot={snapshot} selectedId={selectedId} onRefresh={refresh} />
          </div>
        </aside>
      </div>

      {showExport && <ExportDialog snapshot={snapshot} onClose={() => setShowExport(false)} />}
      {showSettings && (
        <SettingsDialog
          onClose={() => setShowSettings(false)}
          onSettingsChanged={() => {
            api
              .getSettings()
              .then((s) => setModelName(s.model))
              .catch(() => undefined);
          }}
        />
      )}
    </div>
  );
};

const App: React.FC = () => (
  <ToastProvider>
    <Workspace />
  </ToastProvider>
);

export default App;
