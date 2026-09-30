import React, { useEffect, useMemo, useRef, useState } from "react";
import { api, type ModelInfo } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

/**
 * Searchable model dropdown fed by REAL provider discovery
 * (`ai_list_models`, server-cached + TTL). Search filters locally — no
 * network request per keystroke (spec §32).
 */
const ModelSelect: React.FC<{
  value: string;
  disabled?: boolean;
  onChanged: (model: string) => void;
  onListLoaded?: (models: ModelInfo[]) => void;
}> = ({ value, disabled, onChanged, onListLoaded }) => {
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState("");
  const [warning, setWarning] = useState<string | null>(null);
  const [hl, setHl] = useState(0);
  const boxRef = useRef<HTMLDivElement>(null);

  const load = async (refresh: boolean) => {
    setLoading(true);
    try {
      const r = await api.listModels(refresh);
      setModels(r.models);
      setWarning(r.warning ?? null);
      onListLoaded?.(r.models);
    } catch (e) {
      setModels([]);
      setWarning(errMsg(e));
      toast.show(`${t.settings.discoveryFailed} ${errMsg(e)}`, "bad");
    } finally {
      setLoading(false);
    }
  };

  // Initial (cached) fetch on first open.
  useEffect(() => {
    if (open && models === null && !loading) {
      void load(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Close on outside click.
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (!boxRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const filtered = useMemo(() => {
    if (!models) return [];
    const q = query.trim().toLowerCase();
    if (!q) return models;
    return models.filter(
      (m) => m.id.toLowerCase().includes(q) || (m.owned_by ?? "").toLowerCase().includes(q),
    );
  }, [models, query]);

  const pick = (id: string) => {
    onChanged(id);
    setOpen(false);
  };

  return (
    <div className="model-select" ref={boxRef}>
      <button
        className="current"
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{value || "—"}</span>
        <span className="caret">▼</span>
      </button>
      {open && (
        <div className="model-dropdown">
          <input
            className="search"
            type="search"
            autoFocus
            placeholder={t.settings.searchModels}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setHl(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") setHl((h) => Math.min(h + 1, filtered.length - 1));
              if (e.key === "ArrowUp") setHl((h) => Math.max(h - 1, 0));
              if (e.key === "Enter" && filtered[hl]) pick(filtered[hl].id);
              if (e.key === "Escape") setOpen(false);
            }}
          />
          <div className="list" role="listbox">
            {loading && <div className="muted" style={{ padding: "4px 10px" }}>{t.settings.loadingModels}</div>}
            {!loading && !filtered.length && (
              <div className="muted" style={{ padding: "4px 10px" }}>{t.settings.noModels}</div>
            )}
            {!loading &&
              filtered.map((m, i) => (
                <div
                  key={m.id}
                  role="option"
                  aria-selected={m.id === value}
                  className={`opt${i === hl ? " hl" : ""}${m.id === value ? " sel" : ""}`}
                  onClick={() => pick(m.id)}
                  onMouseEnter={() => setHl(i)}
                >
                  <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{m.id}</span>
                  {m.owned_by && <span className="own">{m.owned_by}</span>}
                </div>
              ))}
          </div>
          <div className="row" style={{ padding: "6px 8px", borderTop: "1px solid var(--line-soft)" }}>
            <button className="small" onClick={() => load(true)} disabled={loading}>
              ⟳ {t.settings.refreshModels}
            </button>
            <span className="tiny">
              {loading
                ? t.settings.loadingModels
                : models
                  ? t.settings.modelsCount(models.length)
                  : ""}
            </span>
          </div>
          {warning && (
            <div className="tiny" style={{ padding: "0 10px 8px", color: "var(--warn)" }}>
              {warning}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default ModelSelect;
