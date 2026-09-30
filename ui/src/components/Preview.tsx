import React, { useCallback, useEffect, useRef, useState } from "react";
import { api, assetUrl, type ProjectSnapshot, type SourceInfo } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const Preview: React.FC<{
  snapshot: ProjectSnapshot | null;
  offline: boolean;
  playheadMs: number;
  onSeek: (ms: number) => void;
  previewSource: SourceInfo | null;
}> = ({ snapshot, offline, playheadMs, onSeek, previewSource }) => {
  const toast = useToast();
  const videoRef = useRef<HTMLVideoElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [zoomMode, setZoomMode] = useState<"fit" | "50" | "75" | "100">("fit");
  const [playing, setPlaying] = useState(false);

  // Load the proxy for the active preview target (selected source or first).
  const loadProxy = useCallback(
    async (s: SourceInfo) => {
      setLoading(true);
      try {
        const { path } = await api.proxyPath(s.id);
        setSrc(assetUrl(path));
      } catch (e) {
        toast.show(errMsg(e), "bad");
      } finally {
        setLoading(false);
      }
    },
    [toast],
  );

  const target = previewSource ?? snapshot?.sources[0] ?? null;

  // Auto-load when the target changes and nothing is loaded yet.
  useEffect(() => {
    if (target && !src && !loading && !offline) {
      void loadProxy(target);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target?.id]);

  // Seek the <video> when the timeline playhead moves (mirror, not master).
  useEffect(() => {
    const v = videoRef.current;
    if (!v || !v.duration || Number.isNaN(v.duration)) return;
    // Proxy duration ≈ timeline clip duration for the primary source.
    const frac = Math.min(1, playheadMs / Math.max(1, snapshot?.duration_ms ?? 1));
    const t = frac * v.duration;
    if (Math.abs(v.currentTime - t) > 0.15) {
      v.currentTime = t;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playheadMs]);

  // Report time back to the shared playhead while playing.
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    const onTime = () => {
      if (!v.duration || Number.isNaN(v.duration)) return;
      const ms = (v.currentTime / v.duration) * Math.max(1, snapshot?.duration_ms ?? 1);
      onSeek(Math.round(ms));
    };
    const onPlay = () => setPlaying(true);
    const onPause = () => setPlaying(false);
    v.addEventListener("timeupdate", onTime);
    v.addEventListener("play", onPlay);
    v.addEventListener("pause", onPause);
    return () => {
      v.removeEventListener("timeupdate", onTime);
      v.removeEventListener("play", onPlay);
      v.removeEventListener("pause", onPause);
    };
  }, [src, snapshot?.duration_ms, onSeek]);

  const togglePlay = () => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) {
      v.play().catch(() => undefined);
    } else {
      v.pause();
    }
  };

  const frameStep = (dir: 1 | -1) => {
    const v = videoRef.current;
    if (!v) return;
    v.pause();
    v.currentTime = Math.max(0, Math.min(v.duration || 0, v.currentTime + dir / 25));
  };

  const fullscreen = () => {
    const el = wrapRef.current;
    if (!el) return;
    if (document.fullscreenElement) {
      document.exitFullscreen().catch(() => undefined);
    } else {
      el.requestFullscreen().catch(() => undefined);
    }
  };

  const zoomStyle =
    zoomMode === "fit" ? { maxWidth: "100%", maxHeight: "100%" } : { width: zoomMode === "50" ? "50%" : zoomMode === "75" ? "75%" : "100%" };

  return (
    <div className="preview" ref={wrapRef}>
      {src ? (
        <video ref={videoRef} src={src} style={zoomStyle} onClick={togglePlay} />
      ) : (
        <div className="notice">
          {loading ? t.preview.noProxyYet : t.preview.empty}
        </div>
      )}
      <div className="preview-toolbar">
        {(["fit", "50", "75", "100"] as const).map((m) => (
          <button
            key={m}
            className={zoomMode === m ? "on" : ""}
            onClick={() => setZoomMode(m)}
            title={m === "fit" ? t.preview.fit : t.preview.zoom(Number(m))}
          >
            {m === "fit" ? t.preview.fit : m}
          </button>
        ))}
        <button onClick={fullscreen} title={t.preview.fullscreen}>⛶</button>
      </div>
      <div className="transport" style={{ position: "absolute", bottom: 0, left: 0, right: 0 }}>
        <button onClick={togglePlay} disabled={!src} title={playing ? t.preview.pause : t.preview.play}>
          {playing ? "⏸" : "▶"}
        </button>
        <button onClick={() => frameStep(-1)} disabled={!src} title={t.preview.frameStep}>⏮</button>
        <button onClick={() => frameStep(1)} disabled={!src} title={t.preview.frameStep}>⏭</button>
        <span className="time">
          {(playheadMs / 1000).toFixed(2)}s / {((snapshot?.duration_ms ?? 0) / 1000).toFixed(2)}s
        </span>
      </div>
    </div>
  );
};

export default Preview;
