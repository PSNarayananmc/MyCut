import { useState } from "react";
import { api } from "../lib/bridge";
import { strings as t } from "../i18n/en";

const ExportPanel: React.FC<{ offline: boolean }> = ({ offline }) => {
  const [state, setState] = useState<string | null>(null);
  const doExport = async () => {
    setState("…");
    try {
      const r = await api.renderFinal("");
      setState(`${t.export.done} (${r.seconds.toFixed(1)}s)`);
    } catch (e) {
      setState(`${t.export.failed}: ${String(e)}`);
    }
  };
  return (
    <div className="form">
      <button className="btn btn-primary" onClick={doExport} disabled={offline}>
        {t.export.start}
      </button>
      {state && <div className="badge">{state}</div>}
    </div>
  );
};

export default ExportPanel;
