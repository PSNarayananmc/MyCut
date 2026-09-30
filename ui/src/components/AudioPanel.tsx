import React from "react";
import {
  api,
  type AudioMasterInfo,
  type ProjectSnapshot,
  type TimelineItem,
} from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

/** Audio panel: master chain (real render filters) + per-clip volume. */
const AudioPanel: React.FC<{
  snapshot: ProjectSnapshot | null;
  selectedId: string | null;
  onRefresh: () => void;
}> = ({ snapshot, selectedId, onRefresh }) => {
  const toast = useToast();
  const master: AudioMasterInfo | undefined = snapshot?.audio_master;
  const selectedItem = snapshot?.tracks.flatMap((tr) => tr.items).find((i) => i.id === selectedId) ?? null;

  const patchMaster = async (patch: Record<string, unknown>) => {
    try {
      await api.setAudioMaster(patch);
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const patchVolume = async (v: number) => {
    if (!selectedItem) return;
    try {
      await api.setItemVolume(selectedItem.id, v);
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  return (
    <div>
      <div className="panel-section">
        <h4>{t.audio.master}</h4>
        <div className="stack">
          <label className="row" style={{ gap: 8 }}>
            <input
              type="checkbox"
              checked={!!master?.normalize}
              onChange={(e) => patchMaster({ normalize: e.target.checked })}
            />
            {t.audio.normalize}
          </label>
          <label className="row" style={{ gap: 8 }}>
            <input
              type="checkbox"
              checked={!!master?.denoise}
              onChange={(e) => patchMaster({ denoise: e.target.checked })}
            />
            {t.audio.denoise}
          </label>
          <label className="row" style={{ gap: 8 }}>
            <input
              type="checkbox"
              checked={!!master?.duck_music_under_speech}
              onChange={(e) => patchMaster({ duckMusicUnderSpeech: e.target.checked })}
            />
            {t.audio.duck}
          </label>
          <label>
            {t.audio.fadeIn}: {(master?.fade_in_s ?? 0).toFixed(1)}s
            <input
              type="range"
              min={0}
              max={10}
              step={0.1}
              value={master?.fade_in_s ?? 0}
              onChange={(e) => patchMaster({ fadeInS: Number(e.target.value) })}
            />
          </label>
          <label>
            {t.audio.fadeOut}: {(master?.fade_out_s ?? 0).toFixed(1)}s
            <input
              type="range"
              min={0}
              max={10}
              step={0.1}
              value={master?.fade_out_s ?? 0}
              onChange={(e) => patchMaster({ fadeOutS: Number(e.target.value) })}
            />
          </label>
        </div>
      </div>

      <div className="panel-section">
        <h4>{t.audio.clipVolume}</h4>
        {!selectedItem || !(selectedItem.kind === "video" || selectedItem.kind === "audio") ? (
          <div className="muted">{t.audio.nothingSelected}</div>
        ) : (
          <label>
            {(selectedItem as TimelineItem).label}
            <input
              type="range"
              min={0}
              max={4}
              step={0.05}
              value={(selectedItem as TimelineItem).volume ?? 1}
              onChange={(e) => patchVolume(Number(e.target.value))}
            />
            <span className="val">{(((selectedItem as TimelineItem).volume ?? 1) * 100).toFixed(0)}%</span>
          </label>
        )}
      </div>
    </div>
  );
};

export default AudioPanel;
