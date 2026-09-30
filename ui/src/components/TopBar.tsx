import React, { useState } from "react";
import { api, type ProjectSnapshot } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

export type ConnState = "unknown" | "checking" | "ok" | "bad";

const TopBar: React.FC<{
  snapshot: ProjectSnapshot | null;
  offline: boolean;
  conn: ConnState;
  connText: string;
  onRename: (name: string) => void;
  onRefresh: () => void;
  onExport: () => void;
  onSettings: () => void;
}> = ({ snapshot, offline, conn, connText, onRename, onRefresh, onExport, onSettings }) => {
  const toast = useToast();
  const [saving, setSaving] = useState(false);

  const save = async () => {
    setSaving(true);
    try {
      await api.save();
      toast.show(t.app.savedOk, "ok");
    } catch (e) {
      toast.show(errMsg(e), "bad");
    } finally {
      setSaving(false);
    }
  };

  const dotClass = offline ? "" : conn === "ok" ? "ok" : conn === "checking" ? "busy" : conn === "bad" ? "bad" : "";
  const dotTitle = offline ? t.app.notConnected : connText || t.app.notConnected;

  return (
    <header className="topbar">
      <div className="logo">
        My<span>Cut</span>
      </div>
      <input
        className="project-name"
        value={snapshot?.name ?? ""}
        placeholder="Untitled"
        aria-label="Project name"
        disabled={offline || !snapshot}
        onChange={(e) => onRename(e.target.value)}
        onBlur={() => {
          // Persist the project title through the generic settings path is
          // wrong — the name lives in the project. Save button covers it.
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
        style={{ width: Math.max(90, Math.min(220, (snapshot?.name.length ?? 8) * 9 + 24)) }}
      />
      <button className="ghost" onClick={() => api.undo().then(onRefresh).catch((e) => toast.show(errMsg(e), "bad"))} disabled={offline} title="Ctrl+Z">
        ↶ {t.app.undo}
      </button>
      <button className="ghost" onClick={() => api.redo().then(onRefresh).catch((e) => toast.show(errMsg(e), "bad"))} disabled={offline} title="Ctrl+Shift+Z">
        ↷ {t.app.redo}
      </button>
      <button className="ghost" onClick={save} disabled={offline || saving} title="Ctrl+S">
        {t.app.save}
      </button>
      <div className="spacer" />
      <span className="connection-dot" title={dotTitle}>
        <span className={`dot ${dotClass}`} />
        {offline ? t.app.notConnected : conn === "ok" ? t.app.connected : connText}
      </span>
      <button onClick={onExport} disabled={offline} title="Ctrl+E">
        {t.app.export}
      </button>
      <button className="ghost" onClick={onSettings} aria-label={t.app.settings} title={t.app.settings}>
        ⚙
      </button>
    </header>
  );
};

export default TopBar;
