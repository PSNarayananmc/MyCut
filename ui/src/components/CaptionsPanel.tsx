import React, { useEffect, useState } from "react";
import { api, type ProjectSnapshot } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

interface Entry {
  startMs: number;
  endMs: number;
  text: string;
}

const CaptionsPanel: React.FC<{
  snapshot: ProjectSnapshot | null;
  playheadMs: number;
  onRefresh: () => void;
}> = ({ snapshot, playheadMs, onRefresh }) => {
  const toast = useToast();
  const [styles, setStyles] = useState<{ id: string; label: string }[]>([]);
  const [style, setStyle] = useState("gaming");
  const [entries, setEntries] = useState<Entry[]>([]);

  useEffect(() => {
    api.captionStyleCatalog().then((r) => setStyles(r.styles)).catch(() => undefined);
  }, []);

  const addEntry = () => {
    const start = Math.round(playheadMs);
    setEntries((prev) => [...prev, { startMs: start, endMs: start + 1500, text: "" }]);
  };

  const update = (idx: number, patch: Partial<Entry>) => {
    setEntries((prev) => prev.map((e, i) => (i === idx ? { ...e, ...patch } : e)));
  };

  const apply = async () => {
    const clean = entries.filter((e) => e.text.trim() && e.endMs > e.startMs);
    if (!clean.length) {
      toast.show("Add at least one timed line.", "bad");
      return;
    }
    try {
      await api.setCaptions(
        clean.map((e) => ({ startMs: e.startMs, endMs: e.endMs, text: e.text.trim() })),
        style,
      );
      toast.show("Captions applied", "ok");
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const remove = async () => {
    try {
      await api.removeCaptions();
      toast.show("Captions removed", "ok");
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  return (
    <div>
      <div className="panel-section">
        <h4>{t.captions.style}</h4>
        <div className="row wrap">
          {styles.map((s) => (
            <button key={s.id} className={`small${style === s.id ? " active" : ""}`} onClick={() => setStyle(s.id)}>
              {s.label}
            </button>
          ))}
        </div>
        <div className="tiny" style={{ marginTop: 6 }}>{t.captions.note}</div>
      </div>

      <div className="panel-section">
        <h4>{t.captions.manualTitle}</h4>
        <div className="stack">
          {entries.map((e, i) => (
            <div className="card" key={i}>
              <div className="grid3">
                <label>
                  {t.captions.entryStart}
                  <input
                    type="number"
                    min={0}
                    step={0.1}
                    value={(e.startMs / 1000).toFixed(1)}
                    onChange={(ev) => update(i, { startMs: Math.round(Number(ev.target.value) * 1000) })}
                  />
                </label>
                <label>
                  {t.captions.entryEnd}
                  <input
                    type="number"
                    min={0}
                    step={0.1}
                    value={(e.endMs / 1000).toFixed(1)}
                    onChange={(ev) => update(i, { endMs: Math.round(Number(ev.target.value) * 1000) })}
                  />
                </label>
                <label>
                  {t.captions.entryText}
                  <input
                    type="text"
                    value={e.text}
                    onChange={(ev) => update(i, { text: ev.target.value })}
                  />
                </label>
              </div>
              <button className="small danger" onClick={() => setEntries((prev) => prev.filter((_, j) => j !== i))}>
                Remove line
              </button>
            </div>
          ))}
          <div className="row">
            <button className="small" onClick={addEntry}>{t.captions.addEntry}</button>
          </div>
        </div>
      </div>

      <div className="row">
        <button className="primary" onClick={apply}>{t.captions.apply}</button>
        {!!snapshot?.captions_count && (
          <button className="danger" onClick={remove}>{t.captions.remove}</button>
        )}
      </div>
      {!!snapshot?.captions_count && (
        <div className="tiny" style={{ marginTop: 6 }}>
          {snapshot.captions_count} caption line(s) on the timeline.
        </div>
      )}
    </div>
  );
};

export default CaptionsPanel;
