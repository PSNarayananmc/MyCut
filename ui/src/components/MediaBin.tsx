import type { ProjectSnapshot, SourceInfo } from "../lib/bridge";
import { api } from "../lib/bridge";
import { strings as t } from "../i18n/en";

const fmtDur = (ms: number) => `${(ms / 1000).toFixed(1)}s`;

const MediaBin: React.FC<{
  snapshot: ProjectSnapshot | null;
  onRefresh: () => void;
}> = ({ snapshot, onRefresh }) => {
  const relink = (s: SourceInfo) => {
    api.invoke("relink_source", { sourceId: s.id }).then(onRefresh).catch(() => undefined);
  };
  return (
    <div className="media-list">
      {!snapshot?.sources.length && <div className="media-card meta">{t.timeline.dropHere}</div>}
      {snapshot?.sources.map((s) => (
        <div key={s.id} className={`media-card${s.offline ? " offline" : ""}`}>
          <div className="name">{s.name}</div>
          <div className="meta">
            {fmtDur(s.duration_ms)} · {s.width}×{s.height} · {(s.fps_num / s.fps_den).toFixed(2)} fps
            {s.has_audio ? " · 🔊" : ""}
          </div>
          {s.offline && (
            <div>
              <div className="offline-note">{t.timeline.mediaOffline}</div>
              <button className="btn" onClick={() => relink(s)}>
                {t.timeline.relink}
              </button>
            </div>
          )}
        </div>
      ))}
    </div>
  );
};

export default MediaBin;
