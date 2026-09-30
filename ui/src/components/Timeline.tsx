import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type ProjectSnapshot, type TrackInfo } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const fmt = (ms: number) => {
  const s = Math.max(0, ms) / 1000;
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  return `${m}:${sec.toFixed(1).padStart(4, "0")}`;
};

const kindClass = (kind: string) => kind; // clip video/audio/text/captions/overlay

const srcRange = (it: { kind: string; source_in_ms?: number; source_out_ms?: number; speed?: number }) => ({
  in: it.source_in_ms ?? 0,
  out: it.source_out_ms ?? 0,
  speed: it.speed ?? 1,
});

interface DragState {
  itemId: string;
  mode: "move" | "trim-l" | "trim-r";
  startX: number;
  origStart: number;
  origDur: number;
  origSourceIn: number;
  origSourceOut: number;
  speed: number;
  kind: string;
}

const Timeline: React.FC<{
  snapshot: ProjectSnapshot | null;
  offline: boolean;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  onRefresh: () => void;
  onSeek: (ms: number) => void;
  onSplitAt: (ms: number) => void;
  playheadMs: number;
  zoom: number;
  setZoom: (z: number) => void;
  draggingSourceId: string | null;
}> = ({ snapshot, offline, selectedId, onSelect, onRefresh, onSeek, onSplitAt, playheadMs, zoom, setZoom, draggingSourceId }) => {
  const toast = useToast();
  const scrollRef = useRef<HTMLDivElement>(null);
  const laneRef = useRef<HTMLDivElement>(null);
  const drag = useRef<DragState | null>(null);
  const [snap, setSnap] = useState(true);
  const [laneDragOver, setLaneDragOver] = useState<string | null>(null);

  const pxPerMs = 0.12 * zoom; // 1x ≈ 120 px/s
  const dur = Math.max(snapshot?.duration_ms ?? 0, 1000);
  const widthMs = Math.max(dur + 2000, 8000);
  const width = widthMs * pxPerMs;

  const tracks = snapshot?.tracks ?? [];

  // Adaptive ruler step: keep labels 60+ px apart.
  const tickMs = useMemo(() => {
    const candidates = [100, 250, 500, 1000, 2000, 5000, 10000, 30000, 60000];
    for (const c of candidates) {
      if (c * pxPerMs >= 64) return c;
    }
    return 60000;
  }, [pxPerMs]);

  const snapPoints = useMemo(() => {
    const pts: number[] = [0, playheadMs];
    for (const tr of tracks) {
      for (const it of tr.items) {
        pts.push(it.timeline_start_ms, it.timeline_start_ms + it.timeline_duration_ms);
      }
    }
    return pts;
  }, [tracks, playheadMs]);

  const snapMs = useCallback(
    (ms: number, excludeItem?: string) => {
      if (!snap) return Math.max(0, ms);
      const tolPx = 8;
      const tolMs = tolPx / pxPerMs;
      let best = Math.max(0, ms);
      let bestD = Infinity;
      for (const p of snapPoints) {
        const d = Math.abs(p - ms);
        if (d < tolMs && d < bestD) {
          bestD = d;
          best = p;
        }
      }
      void excludeItem;
      return best;
    },
    [snap, snapPoints, pxPerMs],
  );

  const xToMs = (clientX: number): number => {
    const lane = laneRef.current;
    if (!lane) return 0;
    const rect = lane.getBoundingClientRect();
    return Math.max(0, Math.round((clientX - rect.left) / pxPerMs));
  };

  // ---- playhead seeking on ruler ----
  const rulerSeek = (e: React.MouseEvent) => {
    onSeek(xToMs(e.clientX));
  };

  // ---- clip drag / trim ----
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      const d = drag.current;
      if (!d) return;
      const dxMs = Math.round((e.clientX - d.startX) / pxPerMs);
      window.getSelection()?.removeAllRanges();
      if (d.mode === "move") {
        // moving is applied live via preview translate; commit on mouseup
        const el = document.querySelector<HTMLElement>(`[data-item-id="${d.itemId}"]`);
        if (el) {
          const snapped = snapMs(d.origStart + dxMs);
          el.style.left = `${snapped * pxPerMs}px`;
        }
      } else if (d.mode === "trim-l") {
        const el = document.querySelector<HTMLElement>(`[data-item-id="${d.itemId}"]`);
        if (el) {
          const newStart = Math.min(d.origStart + dxMs, d.origStart + d.origDur - 100);
          const newDur = d.origStart + d.origDur - newStart;
          el.style.left = `${Math.max(0, newStart) * pxPerMs}px`;
          el.style.width = `${Math.max(20, newDur) * pxPerMs}px`;
        }
      } else {
        const el = document.querySelector<HTMLElement>(`[data-item-id="${d.itemId}"]`);
        if (el) {
          const newDur = Math.max(100, d.origDur + dxMs);
          el.style.width = `${newDur * pxPerMs}px`;
        }
      }
    };
    const onUp = async (e: MouseEvent) => {
      const d = drag.current;
      drag.current = null;
      document.body.style.cursor = "";
      if (!d) return;
      const dxMs = Math.round((e.clientX - d.startX) / pxPerMs);
      try {
        if (d.mode === "move") {
          const target = snapMs(d.origStart + dxMs);
          if (target !== d.origStart) {
            await api.moveItem(d.itemId, target);
          }
        } else if (d.kind === "text" || d.kind === "captions" || d.kind === "overlay") {
          // Simple timeline-duration trim for non-source-backed items.
          if (d.mode === "trim-l") {
            const newStart = Math.max(0, Math.min(d.origStart + dxMs, d.origStart + d.origDur - 100));
            const newDur = d.origStart + d.origDur - newStart;
            await api.updateText(d.itemId, { startMs: newStart, durationMs: newDur });
          } else {
            const newDur = Math.max(100, d.origDur + dxMs);
            await api.updateText(d.itemId, { durationMs: newDur });
          }
        } else if (d.mode === "trim-l") {
          const newStart = Math.max(0, Math.min(d.origStart + dxMs, d.origStart + d.origDur - 100));
          const deltaTimeline = newStart - d.origStart;
          const deltaSource = Math.round(deltaTimeline * d.speed);
          const newIn = Math.max(0, d.origSourceIn + deltaSource);
          const newOut = d.origSourceOut;
          const newDur = Math.max(100, Math.round((newOut - newIn) / d.speed));
          await api.trimClip(d.itemId, newIn, newOut, newStart === 0 && newIn > 0 ? d.origStart + deltaTimeline : newStart);
          void newDur;
        } else {
          const newDurTl = Math.max(100, d.origDur + dxMs);
          const newOut = Math.min(
            // the source bound is unknown here; the backend clamps
            d.origSourceIn + Math.round(newDurTl * d.speed),
            Number.MAX_SAFE_INTEGER,
          );
          await api.trimClip(d.itemId, d.origSourceIn, newOut, d.origStart);
        }
        onRefresh();
      } catch (err) {
        toast.show(errMsg(err), "bad");
        onRefresh();
      }
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [pxPerMs, snapMs, onRefresh, toast]);

  const beginDrag = (e: React.MouseEvent, itemId: string, mode: DragState["mode"], item: { timeline_start_ms: number; timeline_duration_ms: number }, sourceRange: { in: number; out: number; speed: number }, kind: string) => {    e.stopPropagation();
    onSelect(itemId);
    drag.current = {
      itemId,
      mode,
      startX: e.clientX,
      origStart: item.timeline_start_ms,
      origDur: item.timeline_duration_ms,
      origSourceIn: sourceRange.in,
      origSourceOut: sourceRange.out,
      speed: sourceRange.speed || 1,
      kind,
    };
    document.body.style.cursor = mode === "move" ? "grabbing" : "ew-resize";
  };

  const dropOnLane = async (e: React.DragEvent, trackKind: string) => {
    e.preventDefault();
    setLaneDragOver(null);
    const sourceId = e.dataTransfer.getData("text/mycut-source") || draggingSourceId;
    if (!sourceId) return;
    const at = snapMs(xToMs(e.clientX));
    try {
      await api.addSourceToTimeline(sourceId, at);
      onRefresh();
    } catch (err) {
      toast.show(errMsg(err), "bad");
    }
    void trackKind;
  };

  const toggleProps = async (tr: TrackInfo, patch: { muted?: boolean; locked?: boolean }) => {
    try {
      await api.setTrackProps(tr.id, patch);
      onRefresh();
    } catch (err) {
      toast.show(errMsg(err), "bad");
    }
  };

  const visibleTracks = tracks;

  return (
    <div className="timeline-wrap">
      <div className="tl-toolbar">
        <button className="small" onClick={() => onSplitAt(playheadMs)} disabled={offline || !selectedId} title={t.timeline.split}>
          ✂ {t.timeline.split}
        </button>
        <button
          className="small"
          disabled={offline || !selectedId}
          onClick={() => selectedId && api.duplicateItem(selectedId).then(onRefresh).catch((e) => toast.show(errMsg(e), "bad"))}
          title="Ctrl+D"
        >
          ⧉ {t.timeline.duplicate}
        </button>
        <button
          className="small danger"
          disabled={offline || !selectedId}
          onClick={() => selectedId && api.deleteItem(selectedId).then(onRefresh).catch((e) => toast.show(errMsg(e), "bad"))}
          title="Del"
        >
          🗑 {t.timeline.delete}
        </button>
        <button className={`small${snap ? " active" : ""}`} onClick={() => setSnap((s) => !s)} title="Snapping">
          🧲
        </button>
        <div className="spacer" />
        <div className="zoom">
          <span className="tiny">−</span>
          <input
            type="range"
            min={0.2}
            max={5}
            step={0.05}
            value={zoom}
            onChange={(e) => setZoom(Number(e.target.value))}
            aria-label={t.timeline.zoom}
          />
          <span className="tiny">+</span>
        </div>
      </div>
      <div className="tl-scroll" ref={scrollRef}>
        <div className="timeline" style={{ width }}>
          <div className="ruler" style={{ width }} onClick={rulerSeek} role="slider" aria-label="Playhead position">
            {Array.from({ length: Math.ceil(widthMs / tickMs) + 1 }, (_, i) => {
              const ms = i * tickMs;
              return (
                <div key={i} className={`tick${i % 5 === 0 ? " major" : ""}`} style={{ left: ms * pxPerMs }}>
                  {i % 5 === 0 ? fmt(ms) : ""}
                </div>
              );
            })}
          </div>
          {visibleTracks.map((tr) => (
            <div className="track-row" key={tr.id}>
              <div className="track-head">
                <div className="t-name">{tr.name}</div>
                <div className="t-actions">
                  <button
                    className={tr.muted ? "on" : ""}
                    onClick={() => toggleProps(tr, { muted: !tr.muted })}
                    title={t.timeline.mute}
                    aria-pressed={tr.muted}
                  >
                    {tr.muted ? "🔇" : "🔊"}
                  </button>
                  <button
                    className={tr.locked ? "lock-on" : ""}
                    onClick={() => toggleProps(tr, { locked: !tr.locked })}
                    title={t.timeline.lock}
                    aria-pressed={tr.locked}
                  >
                    {tr.locked ? "🔒" : "🔓"}
                  </button>
                </div>
              </div>
              <div
                ref={laneRef}
                className={`track-lane${laneDragOver === tr.id ? " dragover" : ""}`}
                onMouseDown={(e) => {
                  if (e.target === e.currentTarget) {
                    onSelect(null);
                    onSeek(xToMs(e.clientX));
                  }
                }}
                onDragOver={(e) => {
                  if (draggingSourceId) {
                    e.preventDefault();
                    setLaneDragOver(tr.id);
                  }
                }}
                onDragLeave={() => setLaneDragOver(null)}
                onDrop={(e) => dropOnLane(e, tr.kind)}
              >
                {tr.items.map((it) => (
                  <div
                    key={it.id}
                    data-item-id={it.id}
                    className={`clip ${kindClass(it.kind)}${selectedId === it.id ? " selected" : ""}`}
                    style={{
                      left: it.timeline_start_ms * pxPerMs,
                      width: Math.max(it.timeline_duration_ms * pxPerMs, 8),
                    }}
                    title={it.label}
                    onMouseDown={(e) => {
                      const target = e.target as HTMLElement;
                      if (target.classList.contains("trim")) return;
                      beginDrag(e, it.id, "move", it, srcRange(it), it.kind);
                    }}
                  >
                    <span
                      className="trim l"
                      onMouseDown={(e) =>
                        beginDrag(e, it.id, "trim-l", it, srcRange(it), it.kind)
                      }
                    />
                    {it.label}
                    <span
                      className="trim r"
                      onMouseDown={(e) =>
                        beginDrag(e, it.id, "trim-r", it, srcRange(it), it.kind)
                      }
                    />
                  </div>
                ))}
                {tr.locked && (
                  <div style={{ position: "absolute", inset: 0, background: "rgba(0,0,0,0.25)", pointerEvents: "none" }} />
                )}
              </div>
            </div>
          ))}
          {!visibleTracks.length && <div className="tl-empty">{t.timeline.dropHere}</div>}
          <div className="playhead" style={{ left: playheadMs * pxPerMs }} />
        </div>
      </div>
      <div className="tiny" style={{ padding: "3px 10px", color: "var(--text-2)" }}>
        {t.timeline.shortcuts}
      </div>
    </div>
  );
};

export default Timeline;
