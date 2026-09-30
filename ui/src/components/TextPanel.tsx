import React, { useState } from "react";
import { api, type ProjectSnapshot, type TimelineItem } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const POSITIONS = [
  ["center", "Center"],
  ["top_left", "Top left"],
  ["top_center", "Top center"],
  ["top_right", "Top right"],
  ["bottom_left", "Bottom left"],
  ["bottom_center", "Bottom center"],
  ["bottom_right", "Bottom right"],
] as const;

const TextPanel: React.FC<{
  snapshot: ProjectSnapshot | null;
  selectedId: string | null;
  onSelect: (id: string) => void;
  playheadMs: number;
  onRefresh: () => void;
}> = ({ snapshot, selectedId, onSelect, playheadMs, onRefresh }) => {
  const toast = useToast();
  const [content, setContent] = useState("");
  const [kind, setKind] = useState("title");
  const [position, setPosition] = useState("bottom_center");
  const [startMs, setStartMs] = useState<number | "">("");
  const [durationMs, setDurationMs] = useState(3000);
  const [scale, setScale] = useState(1);

  const textItems = (snapshot?.tracks.find((tr) => tr.kind === "text")?.items ?? []) as TimelineItem[];

  const add = async () => {
    if (!content.trim()) {
      toast.show("Enter some text first.", "bad");
      return;
    }
    const start = startMs === "" ? Math.round(playheadMs) : startMs;
    try {
      await api.addText(content.trim(), {
        startMs: Math.max(0, start),
        durationMs,
        position,
        kind,
        scale,
      });
      toast.show("Text added", "ok");
      setContent("");
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  return (
    <div>
      <div className="panel-section">
        <h4>{t.text.addTitle}</h4>
        <div className="stack">
          <label>
            {t.text.content}
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              rows={2}
              style={{ width: "100%", resize: "vertical" }}
            />
          </label>
          <div className="grid2">
            <label>
              {t.text.kind}
              <select value={kind} onChange={(e) => setKind(e.target.value)}>
                <option value="title">{t.text.kindTitle}</option>
                <option value="lower_third">{t.text.kindLowerThird}</option>
                <option value="callout">{t.text.kindCallout}</option>
                <option value="watermark">{t.text.kindWatermark}</option>
              </select>
            </label>
            <label>
              {t.text.position}
              <select value={position} onChange={(e) => setPosition(e.target.value)}>
                {POSITIONS.map(([v, l]) => (
                  <option key={v} value={v}>{l}</option>
                ))}
              </select>
            </label>
          </div>
          <div className="grid2">
            <label>
              {t.text.start} ({t.captions.entryStart.replace(" (s)", "")})
              <input
                type="number"
                min={0}
                step={0.1}
                placeholder={(playheadMs / 1000).toFixed(1)}
                value={startMs === "" ? "" : startMs / 1000}
                onChange={(e) =>
                  setStartMs(e.target.value === "" ? "" : Math.round(Number(e.target.value) * 1000))
                }
              />
            </label>
            <label>
              {t.text.duration}
              <input
                type="number"
                min={100}
                step={100}
                value={durationMs}
                onChange={(e) => setDurationMs(Number(e.target.value) || 1000)}
              />
            </label>
          </div>
          <label>
            {t.inspector.scale}: {scale.toFixed(2)}
            <input
              type="range"
              min={0.4}
              max={3}
              step={0.05}
              value={scale}
              onChange={(e) => setScale(Number(e.target.value))}
            />
          </label>
          <button className="primary" onClick={add}>
            {t.text.add}
          </button>
        </div>
      </div>

      <div className="panel-section">
        <h4>{t.text.existing}</h4>
        {!textItems.length && <div className="muted">—</div>}
        <div className="stack">
          {textItems.map((it) => (
            <div
              key={it.id}
              className={`card click${selectedId === it.id ? " selected" : ""}`}
              onClick={() => onSelect(it.id)}
            >
              <div className="tiny" style={{ fontWeight: 600, color: "var(--text-0)" }}>
                {it.text || "(empty)"}
              </div>
              <div className="tiny">
                {(it.timeline_start_ms / 1000).toFixed(1)}s + {(it.timeline_duration_ms / 1000).toFixed(1)}s · {it.position}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
};

export default TextPanel;
