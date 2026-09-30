import React, { useEffect, useState } from "react";
import { api, type EffectDefInfo, type ProjectSnapshot } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const EffectsPanel: React.FC<{
  snapshot: ProjectSnapshot | null;
  selectedId: string | null;
  onRefresh: () => void;
}> = ({ snapshot, selectedId, onRefresh }) => {
  const toast = useToast();
  const [catalog, setCatalog] = useState<{ video: EffectDefInfo[]; audio: EffectDefInfo[] } | null>(null);

  useEffect(() => {
    api.effectCatalog().then(setCatalog).catch(() => undefined);
  }, []);

  const selectedItem = snapshot?.tracks.flatMap((tr) => tr.items).find((i) => i.id === selectedId) ?? null;

  const apply = async (def: EffectDefInfo) => {
    if (!selectedItem) {
      toast.show(t.effects.nothingSelected, "bad");
      return;
    }
    const isAudioFx = def.kind === "audio";
    const targetOk = isAudioFx
      ? selectedItem.kind === "video" || selectedItem.kind === "audio"
      : selectedItem.kind === "video";
    if (!targetOk) {
      toast.show(
        isAudioFx ? "Audio effects apply to audio/video clips." : "Video effects apply to video clips.",
        "bad",
      );
      return;
    }
    try {
      await api.addEffect(selectedItem.id, def.def_id);
      toast.show(t.effects.applied(def.label), "ok");
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const Chip: React.FC<{ def: EffectDefInfo }> = ({ def }) => (
    <div
      className="fx-chip"
      role="button"
      tabIndex={0}
      onClick={() => apply(def)}
      onKeyDown={(e) => e.key === "Enter" && apply(def)}
      title={def.description ?? def.label}
    >
      <span className="fx-name">{def.label}</span>
      {def.description && <span className="fx-desc">{def.description}</span>}
    </div>
  );

  return (
    <div>
      <div className="panel-section">
        <h4>{t.effects.video}</h4>
        <div className="stack">
          {catalog?.video.map((d) => <Chip key={d.def_id} def={d} />)}
          {!catalog && <div className="muted">…</div>}
        </div>
      </div>
      <div className="panel-section">
        <h4>{t.effects.audio}</h4>
        <div className="stack">
          {catalog?.audio.map((d) => <Chip key={d.def_id} def={d} />)}
          {!catalog && <div className="muted">…</div>}
        </div>
      </div>
      <div className="tiny">{t.effects.applyTo}</div>
      {!selectedItem && <div className="tiny" style={{ marginTop: 6, color: "var(--warn)" }}>{t.effects.nothingSelected}</div>}
    </div>
  );
};

export default EffectsPanel;
