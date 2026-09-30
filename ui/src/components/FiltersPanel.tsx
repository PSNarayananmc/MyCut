import React from "react";
import { api, type ColorInfo, type ProjectSnapshot } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const DEFAULTS: ColorInfo = {
  exposure: 0,
  contrast: 1,
  saturation: 1,
  temperature: 0,
  gamma: 1,
  vibrance: 0,
};

const PRESETS: Record<string, Partial<ColorInfo>> = {
  vibrant: { saturation: 1.18, contrast: 1.05, vibrance: 0.15 },
  neutral: {},
  warm: { temperature: 0.12, saturation: 1.05 },
  cinematic: { contrast: 1.12, saturation: 0.92, exposure: -0.03 },
};

/** Color panel: project-level grade — real FFmpeg eq/color filters at render. */
const FiltersPanel: React.FC<{
  snapshot: ProjectSnapshot | null;
  onRefresh: () => void;
}> = ({ snapshot, onRefresh }) => {
  const toast = useToast();
  const color: ColorInfo = snapshot?.color ?? DEFAULTS;

  const patch = async (p: Partial<ColorInfo>) => {
    try {
      await api.setColor(p as Record<string, number>);
      onRefresh();
    } catch (e) {
      toast.show(errMsg(e), "bad");
    }
  };

  const Slider: React.FC<{
    label: string;
    field: keyof ColorInfo;
    min: number;
    max: number;
    step: number;
    fmt?: (v: number) => string;
  }> = ({ label, field, min, max, step, fmt }) => (
    <label>
      {label}: <span className="val">{fmt ? fmt(color[field]) : color[field].toFixed(2)}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={color[field]}
        onChange={(e) => patch({ [field]: Number(e.target.value) })}
      />
    </label>
  );

  return (
    <div>
      <div className="panel-section">
        <h4>{t.filters.title}</h4>
        <div className="row wrap" style={{ marginBottom: 10 }}>
          {Object.entries(PRESETS).map(([name, p]) => (
            <button
              key={name}
              className="small"
              onClick={() =>
                patch({
                  ...DEFAULTS,
                  ...p,
                } as Record<string, number>)
              }
            >
              {name === "vibrant"
                ? t.filters.presetVibrant
                : name === "neutral"
                  ? t.filters.presetNeutral
                  : name === "warm"
                    ? t.filters.presetWarm
                    : t.filters.presetCinematic}
            </button>
          ))}
        </div>
        <div className="stack">
          <Slider label={t.filters.exposure} field="exposure" min={-1} max={1} step={0.01} />
          <Slider label={t.filters.contrast} field="contrast" min={0.5} max={2} step={0.01} />
          <Slider label={t.filters.saturation} field="saturation" min={0} max={3} step={0.01} />
          <Slider label={t.filters.vibrance} field="vibrance" min={-1} max={1} step={0.01} />
          <Slider label={t.filters.temperature} field="temperature" min={-1} max={1} step={0.01} />
          <Slider label={t.filters.gamma} field="gamma" min={0.5} max={2} step={0.01} />
        </div>
      </div>
      <div className="tiny">
        Shorts: open {t.app.export} → pick a 9:16 preset (Smart reframe keeps the subject in frame).
      </div>
    </div>
  );
};

export default FiltersPanel;
