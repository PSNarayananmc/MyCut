import React, { useMemo, useRef, useState } from "react";
import {
  api,
  assetUrl,
  backendKind,
  pickMediaPathsViaTauriDialog,
  uploadFile,
  type ProjectSnapshot,
  type SourceInfo,
} from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { useToast, errMsg } from "./Toast";

const fmtDur = (ms: number) => `${(ms / 1000).toFixed(1)}s`;

type Category = "all" | "video" | "audio" | "image";

const catOf = (s: SourceInfo): Category => {
  if (s.role === "image") return "image";
  if (s.role === "music" || (!s.width && s.height === 0)) return "audio";
  if (s.width > 0) return "video";
  return "audio";
};

// Hidden <input type="file"> is only used for the SERVER backend (browser →
// HTTP upload). Under Tauri we use the native OS picker via tauri-plugin-dialog.
const ACCEPT =
  "video/mp4,video/quicktime,video/x-matroska,video/webm,video/x-msvideo,video/x-m4v," +
  "audio/mpeg,audio/wav,audio/ogg,audio/flac,audio/aac,audio/mp4," +
  "image/png,image/jpeg,image/webm,image/webp,.mp4,.mov,.mkv,.webm,.avi,.m4v,.mp3,.wav,.ogg,.flac,.aac,.m4a,.png,.jpg,.jpeg,.webp";

const MediaPanel: React.FC<{
  snapshot: ProjectSnapshot | null;
  onRefresh: () => void;
  onPreviewSource: (s: SourceInfo) => void;
  onDragStartSource: (s: SourceInfo) => void;
}> = ({ snapshot, onRefresh, onPreviewSource, onDragStartSource }) => {
  const toast = useToast();
  const fileInput = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [cat, setCat] = useState<Category>("all");
  const [dragOver, setDragOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number; source: SourceInfo } | null>(null);

  const isTauri = backendKind() === "tauri";
  const sources = snapshot?.sources ?? [];
  const filtered = useMemo(
    () =>
      sources.filter((s) => {
        if (cat !== "all" && catOf(s) !== cat) return false;
        if (query && !s.name.toLowerCase().includes(query.toLowerCase())) return false;
        return true;
      }),
    [sources, cat, query],
  );

  // SERVER backend: pick via hidden <input type="file">, upload each File
  // through the streaming route.
  const importFilesViaServer = async (files: FileList | File[]) => {
    const list = Array.from(files);
    if (!list.length) return;
    setBusy(true);
    let failed = 0;
    for (const f of list) {
      try {
        await uploadFile(f);
      } catch (e) {
        failed++;
        toast.show(`${t.media.importFailed} — ${f.name}: ${errMsg(e)}`, "bad");
      }
    }
    setBusy(false);
    if (list.length - failed > 0) {
      toast.show(`${list.length - failed} file(s) imported`, "ok");
    }
    onRefresh();
  };

  // TAURI backend: open the native OS file picker, pass the picked paths
  // to Rust, which probes + thumbnails + imports them (spec §13).
  const importViaTauriDialog = async () => {
    setBusy(true);
    try {
      const paths = await pickMediaPathsViaTauriDialog();
      if (paths.length === 0) {
        // User cancelled the picker; no toast.
        setBusy(false);
        return;
      }
      const r = await api.importMedia(paths);
      const imported = (r as { imported?: number }).imported ?? 0;
      const skipped = (r as { skipped?: { path: string; reason: string }[] }).skipped ?? [];
      if (imported > 0) {
        toast.show(`${imported} file(s) imported`, "ok");
      }
      for (const s of skipped) {
        toast.show(`${t.media.importFailed} — ${s.path}: ${s.reason}`, "bad");
      }
      onRefresh();
    } catch (e) {
      toast.show(`${t.media.importFailed}: ${errMsg(e)}`, "bad");
    } finally {
      setBusy(false);
    }
  };

  const onImportClick = () => {
    if (isTauri) {
      void importViaTauriDialog();
    } else {
      fileInput.current?.click();
    }
  };

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    // Under Tauri, dropped files from the host file manager expose the
    // path via `path` (non-standard, available in Tauri's webview). Under
    // the server backend, only the File bytes are available.
    const dt = e.dataTransfer;
    if (!dt) return;
    if (isTauri) {
      // Tauri exposes file paths on File objects via the non-standard `.path`
      // property. Collect them and route through `import_media`.
      const paths: string[] = [];
      for (let i = 0; i < dt.files.length; i++) {
        const f = dt.files[i] as File & { path?: string };
        if (typeof f.path === "string" && f.path) paths.push(f.path);
      }
      if (paths.length > 0) {
        setBusy(true);
        api
          .importMedia(paths)
          .then((r) => {
            const imported = (r as { imported?: number }).imported ?? 0;
            if (imported > 0) toast.show(`${imported} file(s) imported`, "ok");
            onRefresh();
          })
          .catch((e) => toast.show(`${t.media.importFailed}: ${errMsg(e)}`, "bad"))
          .finally(() => setBusy(false));
      }
    } else if (dt.files?.length) {
      void importFilesViaServer(dt.files);
    }
  };

  const closeMenu = () => setMenu(null);

  return (
    <div
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("Files")) {
          e.preventDefault();
          setDragOver(true);
        }
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={onDrop}
      onClick={menu ? closeMenu : undefined}
      style={{ height: "100%", display: "flex", flexDirection: "column" }}
    >
      <div className="media-actions">
        <button
          className="primary"
          onClick={onImportClick}
          disabled={busy}
          title={isTauri ? "Open the system file picker (multi-select)" : "Open file picker (multi-select)"}
        >
          {busy ? t.media.importing(1) : `+ ${t.media.import}`}
        </button>
        {/* Hidden input — only used for the SERVER backend (browser upload). */}
        {!isTauri && (
          <input
            ref={fileInput}
            type="file"
            multiple
            accept={ACCEPT}
            style={{ display: "none" }}
            onChange={(e) => {
              if (e.target.files) void importFilesViaServer(e.target.files);
              e.target.value = "";
            }}
          />
        )}
      </div>
      <input
        className="media-search"
        type="search"
        placeholder={t.media.search}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        aria-label={t.media.search}
      />
      <div className="cat-tabs">
        {(["all", "video", "audio", "image"] as Category[]).map((c) => (
          <button key={c} className={cat === c ? "active" : ""} onClick={() => setCat(c)}>
            {c === "all" ? t.media.all : c === "video" ? t.media.videos : c === "audio" ? t.media.audio : t.media.images}
          </button>
        ))}
      </div>

      <div className="media-grid">
        {!sources.length && (
          <div className={`drop-hint${dragOver ? " drag" : ""}`}>
            {t.media.dropHere}
            <div className="tiny" style={{ marginTop: 4 }}>
              {t.media.doubleClickPreview}
            </div>
          </div>
        )}
        {sources.length > 0 && !filtered.length && <div className="muted">No matches.</div>}
        {filtered.map((s) => (
          <div
            key={s.id}
            className={`media-card${s.offline ? " offline" : ""}`}
            draggable={!s.offline}
            onDragStart={(e) => {
              onDragStartSource(s);
              e.dataTransfer.setData("text/mycut-source", s.id);
              e.dataTransfer.effectAllowed = "copy";
            }}
            onDoubleClick={() => onPreviewSource(s)}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, source: s });
            }}
            title={s.name}
          >
            {catOf(s) === "image" ? (
              <img src={assetUrl(s.rel_path)} alt="" loading="lazy" />
            ) : (
              <img
                src={assetUrl(`${s.id}.jpg`)}
                alt=""
                loading="lazy"
                onError={(ev) => {
                  const el = ev.currentTarget;
                  const ph = document.createElement("div");
                  ph.className = "thumb-ph";
                  ph.textContent = catOf(s) === "audio" ? "♪" : "▶";
                  el.replaceWith(ph);
                }}
              />
            )}
            <div>
              <div className="name">{s.name}</div>
              <div className="meta">
                {catOf(s) === "audio"
                  ? `${fmtDur(s.duration_ms)}${s.has_audio ? " · 🔊" : ""}`
                  : `${fmtDur(s.duration_ms)} · ${s.width}×${s.height}`}
              </div>
              {s.offline && <div className="tiny" style={{ color: "var(--warn)" }}>{t.media.offlineNote}</div>}
            </div>
            <button
              className="menu-btn"
              aria-label={t.media.properties}
              onClick={(e) => {
                e.stopPropagation();
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                setMenu({ x: r.left, y: r.bottom + 2, source: s });
              }}
            >
              ⋮
            </button>
          </div>
        ))}
      </div>

      {menu && (
        <div className="ctx-menu" style={{ left: menu.x, top: menu.y }} onClick={(e) => e.stopPropagation()}>
          <button onClick={() => { closeMenu(); onPreviewSource(menu.source); }}>
            {t.media.properties} / {t.panels.preview}
          </button>
          <button
            onClick={() => {
              closeMenu();
              api
                .addSourceToTimeline(menu.source.id, snapshot?.duration_ms ?? 0)
                .then(onRefresh)
                .catch((e) => toast.show(errMsg(e), "bad"));
            }}
          >
            {t.media.addToTimeline}
          </button>
          <button
            onClick={() => {
              closeMenu();
              api.revealSource(menu.source.id).catch((e) => toast.show(errMsg(e), "bad"));
            }}
          >
            {t.media.reveal}
          </button>
          <button
            className="danger"
            onClick={() => {
              closeMenu();
              api
                .removeSource(menu.source.id)
                .then(onRefresh)
                .catch((e) => toast.show(errMsg(e), "bad"));
            }}
          >
            {t.media.remove}
          </button>
        </div>
      )}
      {backendKind() === "none" && <div className="tiny" style={{ marginTop: 8 }}>{t.errors.backendOffline}</div>}
    </div>
  );
};

export default MediaPanel;
