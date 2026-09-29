import type { ProjectSnapshot } from "../lib/bridge";
import { strings as t } from "../i18n/en";

const TimelineView: React.FC<{
  snapshot: ProjectSnapshot | null;
  onRefresh?: () => void;
}> = ({ snapshot, onRefresh }) => {
  const dur = Math.max(snapshot?.duration_ms ?? 0, 1);
  const pxPerMs = 100 / dur;
  return (
    <div className="timeline" onClick={onRefresh}>
      {snapshot?.tracks
        .filter((tr) => tr.items.length > 0)
        .map((tr) => (
          <div className="track" key={tr.id}>
            <div className="track-label">{tr.name}</div>
            <div className="track-lane">
              {tr.items.map((it) => (
                <div
                  key={it.id}
                  className={`clip ${it.kind}`}
                  style={{ left: `${it.timeline_start_ms * pxPerMs}%`, width: `${Math.max(it.timeline_duration_ms * pxPerMs, 1)}%` }}
                  title={it.label}
                >
                  {it.label}
                </div>
              ))}
            </div>
          </div>
        ))}
      {!snapshot?.tracks.some((tr) => tr.items.length > 0) && (
        <div className="track">
          <div className="track-label" />
          <div className="track-lane" style={{ display: "flex", alignItems: "center", justifyContent: "center", color: "#8b93a5", fontSize: 12 }}>
            {t.timeline.dropHere}
          </div>
        </div>
      )}
    </div>
  );
};

export default TimelineView;
