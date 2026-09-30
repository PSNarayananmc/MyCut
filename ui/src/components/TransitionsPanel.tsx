import React, { useEffect, useMemo, useState } from "react";
import { api, type ProjectSnapshot } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const TransitionsPanel: React.FC<{
  snapshot: ProjectSnapshot | null;
  onRefresh: () => void;
}> = ({ snapshot, onRefresh }) => {
  const toast = useToast();
  const [kinds, setKinds] = useState<{ id: string; label: string }[]>([]);
  const [durations, setDurations] = useState<Record<number, number>>({});
  const videoTrack = useMemo(
    () => snapshot?.tracks.find((tr) => tr.kind === "video") ?? null,
    [snapshot],
  );
  const clipCount = videoTrack?.items.length ?? 0;

  useEffect(() => {
    api.transitionCatalog().then((r) => setKinds(r.kinds)).catch(() => undefined);
  }, []);

  const videoItems = videoTrack?.items ?? [];

  const setTransition = async (afterIndex: number, kind: string) => {
    try {
      await api.setTransitions([
        { afterIndex, kind, durationMs: durations[afterIndex] ?? 500 },
      ]);
      toast.show(`Transition set: ${kind}`, "ok");
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  if (clipCount < 2) {
    return (
      <div className="muted">
        {t.transitions.note}
        <div className="tiny" style={{ marginTop: 6 }}>
          Add at least two clips on the main video track.
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="tiny" style={{ marginBottom: 8 }}>{t.transitions.note}</div>
      {videoItems.slice(0, -1).map((it, idx) => (
        <div className="card" key={it.id} style={{ marginBottom: 8 }}>
          <div className="tiny" style={{ marginBottom: 6 }}>
            {t.transitions.between(idx + 1)} — {it.label}
          </div>
          <div className="row wrap">
            {kinds.map((k) => (
              <button key={k.id} className="small" onClick={() => setTransition(idx, k.id)}>
                {k.label}
              </button>
            ))}
          </div>
          <div className="row" style={{ marginTop: 6 }}>
            <label style={{ flex: 1 }}>
              {t.transitions.duration}
              <input
                type="number"
                min={100}
                max={4000}
                step={100}
                value={durations[idx] ?? 500}
                onChange={(e) =>
                  setDurations((d) => ({ ...d, [idx]: Number(e.target.value) || 500 }))
                }
              />
            </label>
          </div>
        </div>
      ))}
    </div>
  );
};

export default TransitionsPanel;
