import React, { useState } from "react";
import {
  api,
  type EffectDefInfo,
  type ProjectSnapshot,
  type TimelineItem,
} from "../lib/bridge";
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

const Inspector: React.FC<{
  snapshot: ProjectSnapshot | null;
  selectedId: string | null;
  onRefresh: () => void;
}> = ({ snapshot, selectedId, onRefresh }) => {
  const toast = useToast();
  const [catalog, setCatalog] = useState<EffectDefInfo[] | null>(null);
  const [textContent, setTextContent] = useState<string | null>(null);

  React.useEffect(() => {
    api
      .effectCatalog()
      .then((c) => setCatalog([...c.video, ...c.audio]))
      .catch(() => undefined);
  }, []);

  React.useEffect(() => {
    const item = findItem();
    setTextContent(item?.text ?? null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId, snapshot?.duration_ms]);

  const findItem = (): TimelineItem | null =>
    snapshot?.tracks.flatMap((tr) => tr.items).find((i) => i.id === selectedId) ?? null;

  const item = findItem();

  if (!item) {
    return <div className="inspector-empty">{t.inspector.empty}</div>;
  }

  const guard = async (fn: () => Promise<unknown>) => {
    try {
      await fn();
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const labelOf = (defId: string) => catalog?.find((d) => d.def_id === defId)?.label ?? defId;
  const paramsOf = (defId: string) => catalog?.find((d) => d.def_id === defId)?.params ?? [];

  const isClip = item.kind === "video" || item.kind === "audio";

  return (
    <div>
      <div className="card" style={{ marginBottom: 10 }}>
        <div className="tiny" style={{ fontWeight: 700, color: "var(--text-0)", fontSize: 12 }}>
          {item.label}
        </div>
        <div className="tiny">{item.kind}</div>
      </div>

      <div className="panel-section">
        <h4>{t.inspector.timing}</h4>
        <div className="grid2 tiny">
          <span>{t.inspector.start}: {(item.timeline_start_ms / 1000).toFixed(2)}s</span>
          <span>{t.inspector.duration}: {(item.timeline_duration_ms / 1000).toFixed(2)}s</span>
          {isClip && item.source_in_ms !== undefined && (
            <span>
              {t.inspector.sourceRange}: {((item.source_in_ms ?? 0) / 1000).toFixed(2)}–
              {((item.source_out_ms ?? 0) / 1000).toFixed(2)}s
            </span>
          )}
        </div>
      </div>

      {item.kind === "text" && (
        <div className="panel-section">
          <h4>{t.inspector.textContent}</h4>
          <div className="stack">
            <textarea
              value={textContent ?? item.text ?? ""}
              rows={2}
              onChange={(e) => setTextContent(e.target.value)}
              style={{ width: "100%", resize: "vertical" }}
            />
            <div className="grid2">
              <label>
                {t.inspector.position}
                <select
                  value={item.position ?? "center"}
                  onChange={(e) => guard(() => api.updateText(item.id, { position: e.target.value }))}
                >
                  {POSITIONS.map(([v, l]) => (
                    <option key={v} value={v}>{l}</option>
                  ))}
                </select>
              </label>
              <label>
                {t.inspector.scale}: {(item.scale ?? 1).toFixed(2)}
                <input
                  type="range"
                  min={0.4}
                  max={3}
                  step={0.05}
                  value={item.scale ?? 1}
                  onChange={(e) => guard(() => api.updateText(item.id, { scale: Number(e.target.value) }))}
                />
              </label>
            </div>
            <button className="small primary" onClick={() => guard(() => api.updateText(item.id, { text: textContent ?? "" }))}>
              {t.inspector.update}
            </button>
          </div>
        </div>
      )}

      {isClip && (
        <>
          <div className="panel-section">
            <h4>{t.inspector.actions}</h4>
            <label>
              {t.inspector.speed}: {(item.speed ?? 1).toFixed(2)}×
              <input
                type="range"
                min={0.25}
                max={4}
                step={0.05}
                value={item.speed ?? 1}
                onChange={(e) => guard(() => api.setClipSpeed(item.id, Number(e.target.value)))}
              />
            </label>
            <label style={{ marginTop: 8 }}>
              {t.inspector.volume}: {((item.volume ?? 1) * 100).toFixed(0)}%
              <input
                type="range"
                min={0}
                max={4}
                step={0.05}
                value={item.volume ?? 1}
                onChange={(e) => guard(() => api.setItemVolume(item.id, Number(e.target.value)))}
              />
            </label>
          </div>

          <div className="panel-section">
            <h4>{t.inspector.effects}</h4>
            {!item.effects.length && <div className="muted">—</div>}
            <div className="stack">
              {item.effects.map((fx) => (
                <div className="card" key={fx.id}>
                  <div className="row" style={{ justifyContent: "space-between" }}>
                    <b className="tiny" style={{ fontSize: 12, color: "var(--text-0)" }}>{labelOf(fx.def_id)}</b>
                    <button
                      className="small danger"
                      onClick={() => guard(() => api.removeEffect(item.id, fx.id))}
                      title={t.inspector.removeEffect}
                    >
                      ✕
                    </button>
                  </div>
                  {paramsOf(fx.def_id).map((p) => {
                    const current = Number(fx.params[p.name] ?? p.default);
                    return (
                      <label key={p.name} style={{ marginTop: 4 }}>
                        {p.label}: <span className="val">{current.toFixed(2)}</span>
                        <input
                          type="range"
                          min={p.min}
                          max={p.max}
                          step={p.step}
                          value={current}
                          onChange={(e) =>
                            guard(() =>
                              api.setEffectParam(item.id, fx.id, p.name, Number(e.target.value)),
                            )
                          }
                        />
                      </label>
                    );
                  })}
                </div>
              ))}
            </div>
          </div>
        </>
      )}
    </div>
  );
};

export default Inspector;
