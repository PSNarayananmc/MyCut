import React, { useEffect, useRef, useState } from "react";
import {
  api,
  assetUrl,
  backendKind,
  type ExportJobStatus,
  type ExportPresetInfo,
  type ProjectSnapshot,
} from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const ExportDialog: React.FC<{
  snapshot: ProjectSnapshot | null;
  onClose: () => void;
}> = ({ snapshot, onClose }) => {
  const toast = useToast();
  const [presets, setPresets] = useState<ExportPresetInfo[]>([]);
  const [presetId, setPresetId] = useState("youtube");
  const [form, setForm] = useState({
    width: 1920,
    height: 1080,
    fps: 30,
    quality: 20,
    audioBitrateKbps: 192,
    videoCodec: "h264",
    audioCodec: "aac",
  });
  const [job, setJob] = useState<ExportJobStatus | null>(null);
  const [outPath, setOutPath] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const pollRef = useRef<number | null>(null);

  const hasClips = !!snapshot?.tracks.some((tr) => tr.items.length > 0);

  // Load presets + current export settings.
  useEffect(() => {
    api
      .exportPresets()
      .then((r) => {
        setPresets(r.presets);
        // Match current project export settings to a preset if possible.
        const cur = snapshot?.export;
        if (cur) {
          setForm({
            width: cur.width,
            height: cur.height,
            fps: cur.fps,
            quality: cur.quality,
            audioBitrateKbps: cur.audio_bitrate_kbps,
            videoCodec: cur.video_codec,
            audioCodec: cur.audio_codec,
          });
          const match = r.presets.find(
            (p) =>
              p.settings.width === cur.width &&
              p.settings.height === cur.height &&
              p.settings.quality === cur.quality,
          );
          if (match) setPresetId(match.id);
        }
      })
      .catch((e) => toast.show(errMsg(e), "bad"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Poll the real job status while running.
  useEffect(() => {
    if (!job?.running) return;
    pollRef.current = window.setInterval(async () => {
      try {
        const s = await api.exportStatus();
        setJob(s);
        if (!s.running && s.done && !s.error && s.path) {
          setOutPath(s.path);
        }
      } catch {
        /* transient */
      }
    }, 500);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
    };
  }, [job?.running]);

  const applyPreset = (id: string) => {
    setPresetId(id);
    const p = presets.find((x) => x.id === id);
    if (!p || id === "custom") return;
    setForm({
      width: p.settings.width,
      height: p.settings.height,
      fps: p.settings.fps,
      quality: p.settings.quality,
      audioBitrateKbps: p.settings.audio_bitrate_kbps,
      videoCodec: p.settings.video_codec,
      audioCodec: p.settings.audio_codec,
    });
  };

  const start = async () => {
    if (!hasClips) {
      toast.show(t.export.empty, "bad");
      return;
    }
    setStarting(true);
    try {
      await api.setExportSettings({
        width: form.width,
        height: form.height,
        fps: form.fps,
        quality: form.quality,
        audioBitrateKbps: form.audioBitrateKbps,
        videoCodec: form.videoCodec,
        audioCodec: form.audioCodec,
      });
      await api.exportStart();
      setJob(await api.exportStatus());
    } catch (e) {
      toast.show(`${t.export.failed}: ${errMsg(e)}`, "bad");
    } finally {
      setStarting(false);
    }
  };

  const cancel = async () => {
    try {
      await api.exportCancel();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const pct = Math.round((job?.progress ?? 0) * 100);

  return (
    <div className="modal-back" onMouseDown={(e) => e.target === e.currentTarget && !job?.running && onClose()}>
      <div className="modal" role="dialog" aria-label={t.export.title}>
        <div className="modal-head">
          {t.export.title}
          <button className="ghost" onClick={onClose} disabled={job?.running} aria-label="Close">✕</button>
        </div>
        <div className="modal-body">
          {!hasClips && <div className="status-line bad">{t.export.empty}</div>}

          {!job || (!job.running && !job.done) ? (
            <>
              <label>
                {t.export.preset}
                <select value={presetId} onChange={(e) => applyPreset(e.target.value)}>
                  {presets.map((p) => (
                    <option key={p.id} value={p.id}>{p.label}</option>
                  ))}
                </select>
              </label>
              <div className="grid3">
                <label>
                  W
                  <input
                    type="number"
                    min={16}
                    max={1920}
                    value={form.width}
                    onChange={(e) => setForm((f) => ({ ...f, width: Number(e.target.value) || 1920, ...(presetId !== "custom" ? {} : {}) }))}
                    disabled={presetId !== "custom"}
                  />
                </label>
                <label>
                  H
                  <input
                    type="number"
                    min={16}
                    max={1920}
                    value={form.height}
                    onChange={(e) => setForm((f) => ({ ...f, height: Number(e.target.value) || 1080 }))}
                    disabled={presetId !== "custom"}
                  />
                </label>
                <label>
                  {t.export.fps}
                  <input
                    type="number"
                    min={12}
                    max={60}
                    value={form.fps}
                    onChange={(e) => setForm((f) => ({ ...f, fps: Number(e.target.value) || 30 }))}
                    disabled={presetId !== "custom"}
                  />
                </label>
              </div>
              <label>
                {t.export.quality} (CRF {form.quality})
                <input
                  type="range"
                  min={14}
                  max={32}
                  step={1}
                  value={form.quality}
                  onChange={(e) => setForm((f) => ({ ...f, quality: Number(e.target.value) }))}
                  disabled={presetId !== "custom"}
                />
              </label>
              <div className="grid2">
                <label>
                  {t.export.videoCodec}
                  <select
                    value={form.videoCodec}
                    onChange={(e) => setForm((f) => ({ ...f, videoCodec: e.target.value }))}
                    disabled={presetId !== "custom"}
                  >
                    <option value="h264">H.264</option>
                    <option value="hevc">HEVC</option>
                    <option value="vp9">VP9</option>
                  </select>
                </label>
                <label>
                  {t.export.audioBitrate}
                  <input
                    type="number"
                    min={64}
                    max={320}
                    step={32}
                    value={form.audioBitrateKbps}
                    onChange={(e) => setForm((f) => ({ ...f, audioBitrateKbps: Number(e.target.value) || 192 }))}
                    disabled={presetId !== "custom"}
                  />
                </label>
              </div>
            </>
          ) : null}

          {job?.running && (
            <div className="stack">
              <div className="status-line busy">{t.export.rendering} {pct}%</div>
              <div className="progressbar">
                <div style={{ width: `${pct}%` }} />
              </div>
            </div>
          )}

          {job && !job.running && job.done && job.error && (
            <div className="status-line bad">
              {job.cancelled ? t.export.cancelled : `${t.export.failed}: ${job.error}`}
            </div>
          )}

          {(outPath || job?.path) && !job?.running && (
            <div className="status-line ok">
              ✓ {t.export.done}
              <div className="tiny" style={{ marginTop: 4, wordBreak: "break-all" }}>
                {outPath ?? job?.path}
              </div>
              {backendKind() === "server" && (
                <div style={{ marginTop: 6 }}>
                  <a href={assetUrl(outPath ?? job?.path ?? "")} download>
                    {t.export.download}
                  </a>
                </div>
              )}
            </div>
          )}
        </div>
        <div className="modal-foot">
          {job?.running ? (
            <button className="danger" onClick={cancel}>{t.export.cancel}</button>
          ) : (
            <>
              <button onClick={onClose}>Close</button>
              <button className="primary" onClick={start} disabled={starting || !hasClips}>
                {t.export.start}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
};

export default ExportDialog;
