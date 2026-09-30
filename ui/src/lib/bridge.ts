/**
 * Typed bridge to the Rust core. Two transports, one command surface:
 * - Tauri app      → `invoke` over Tauri IPC (`__TAURI_INTERNALS__`)
 * - Local web run  → `POST /api/<cmd>` JSON to the loopback server
 *                    (`mycut` binary — the Ubuntu 20.04 runtime)
 * - Plain file:// or `vite dev` without backend → honest "backend
 *   unavailable" state. Nothing is faked.
 */

export interface SourceInfo {
  id: string;
  name: string;
  rel_path: string;
  duration_ms: number;
  width: number;
  height: number;
  fps_num: number;
  fps_den: number;
  has_audio: boolean;
  role: string;
  offline?: boolean;
}

export interface EffectParamInfo {
  name: string;
  label: string;
  default: number;
  min: number;
  max: number;
  step: number;
}

export interface EffectDefInfo {
  def_id: string;
  label: string;
  description?: string;
  kind: "video" | "audio";
  params: EffectParamInfo[];
}

export interface EffectInstanceInfo {
  id: string;
  def_id: string;
  params: Record<string, number | string>;
}

export interface TimelineItem {
  id: string;
  kind: string;
  timeline_start_ms: number;
  timeline_duration_ms: number;
  label: string;
  volume: number;
  opacity: number;
  effects: EffectInstanceInfo[];
  source_id?: string;
  source_in_ms?: number;
  source_out_ms?: number;
  speed?: number;
  text?: string;
  text_kind?: string;
  position?: string;
  scale?: number;
  caption_style?: string;
}

export interface TrackInfo {
  id: string;
  kind: string;
  name: string;
  muted: boolean;
  locked: boolean;
  items: TimelineItem[];
}

export interface ColorInfo {
  exposure: number;
  contrast: number;
  saturation: number;
  temperature: number;
  gamma: number;
  vibrance: number;
}

export interface AudioMasterInfo {
  normalize: boolean;
  denoise: boolean;
  duck_music_under_speech: boolean;
  fade_in_s: number;
  fade_out_s: number;
}

export interface ExportSettingsInfo {
  width: number;
  height: number;
  fps: number;
  video_codec: string;
  audio_codec: string;
  container: string;
  quality: number;
  audio_bitrate_kbps: number;
}

export interface ProjectSnapshot {
  name: string;
  sources: SourceInfo[];
  tracks: TrackInfo[];
  duration_ms: number;
  color_set: boolean;
  color: ColorInfo;
  audio_master: AudioMasterInfo;
  reframe?: { ratio: string; mode: string } | null;
  export: ExportSettingsInfo;
  captions_count: number;
}

export interface ChatTurn {
  role: "user" | "assistant" | "system";
  content: string;
  pending?: boolean;
}

export interface AiDiff {
  summary_parts: string[];
  warnings: string[];
  repairs: string[];
  clarification?: string;
}

export interface ModelInfo {
  id: string;
  owned_by?: string;
  created?: number;
}

export interface AppSettingsInfo {
  baseUrl: string;
  model: string;
  framesEnabled: boolean;
  profile: string;
  cacheLimitBytes?: number;
  keyStored?: boolean;
  keyBackend?: string;
}

export interface ConnectionResult {
  ok: boolean;
  step: "key" | "base_url" | "models" | "chat";
  state: string;
  model_count?: number;
}

export interface ExportJobStatus {
  running: boolean;
  done: boolean;
  progress: number;
  error?: string | null;
  path?: string | null;
  cancelled?: boolean;
}

export interface ExportPresetInfo {
  id: string;
  label: string;
  settings: {
    width: number;
    height: number;
    fps: number;
    video_codec: string;
    audio_codec: string;
    container: string;
    quality: number;
    audio_bitrate_kbps: number;
  };
}

export type BackendKind = "tauri" | "server" | "none";

type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** Real Tauri IPC when available; undefined elsewhere. */
function tauriInvoke(): InvokeFn | null {
  const w = window as unknown as { __TAURI_INTERNALS__?: { invoke: InvokeFn } };
  if (w.__TAURI_INTERNALS__?.invoke) {
    return w.__TAURI_INTERNALS__.invoke;
  }
  return null;
}

export function backendKind(): BackendKind {
  if (tauriInvoke()) return "tauri";
  if (typeof window !== "undefined" && window.location.protocol.startsWith("http")) {
    return "server";
  }
  return "none";
}

export const backendAvailable = (): boolean => backendKind() !== "none";

async function serverInvoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  const resp = await fetch(`api/${cmd}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-MyCut": "1" },
    body: JSON.stringify(args ?? {}),
    credentials: "omit",
    cache: "no-store",
  });
  let payload: unknown = null;
  try {
    payload = await resp.json();
  } catch {
    /* non-JSON body */
  }
  if (!resp.ok) {
    const msg =
      payload && typeof payload === "object" && "error" in payload
        ? String((payload as { error: unknown }).error)
        : `HTTP ${resp.status}`;
    throw new Error(msg);
  }
  return payload;
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const kind = backendKind();
  if (kind === "tauri") {
    return tauriInvoke()!(cmd, args) as Promise<T>;
  }
  if (kind === "server") {
    return serverInvoke(cmd, args) as Promise<T>;
  }
  throw new Error("backend-unavailable");
}

/**
 * Upload one picked/dropped File through the streaming route. Returns the
 * created source id. Progress events are per-file start/end only (the
 * browser does not give us byte-level progress for fetch bodies).
 *
 * Note: this only works for the SERVER backend (browser → HTTP upload).
 * Under Tauri, the webview cannot turn a File object back into a path,
 * so the MediaPanel uses `pickMediaPathsViaTauriDialog()` + `importMedia`
 * instead of this function.
 */
export async function uploadFile(file: File): Promise<{ source: SourceInfo }> {
  const kind = backendKind();
  if (kind === "tauri") {
    // Should never be called under Tauri — MediaPanel routes through the
    // dialog-plugin path. If we end up here anyway, surface an honest error
    // instead of silently dropping the file (spec §34: no fake functionality).
    throw new Error(
      "Under the Tauri shell, file uploads must go through the native picker (use pickMediaPathsViaTauriDialog).",
    );
  }
  const resp = await fetch(`api/import_upload?name=${encodeURIComponent(file.name)}`, {
    method: "POST",
    headers: { "X-MyCut": "1", "Content-Type": "application/octet-stream" },
    body: file,
    credentials: "omit",
    cache: "no-store",
  });
  const payload = (await resp.json().catch(() => null)) as
    | { error?: string; source?: SourceInfo }
    | null;
  if (!resp.ok || !payload || payload.error) {
    throw new Error(payload?.error ?? `HTTP ${resp.status}`);
  }
  return { source: payload.source! };
}

/** Media file extensions accepted by the picker (spec §13). */
const MEDIA_EXTS = [
  "mp4", "mov", "mkv", "webm", "avi", "m4v",
  "mp3", "wav", "ogg", "flac", "aac", "m4a",
  "png", "jpg", "jpeg", "webp",
];

/**
 * Open the native OS file picker via `tauri-plugin-dialog` and return the
 * picked paths. Returns an empty array if the user cancelled or the dialog
 * plugin isn't available. The dialog plugin is injected on `window.__TAURI_INTERNALS__`
 * by `tauri-plugin-dialog` (already a Rust-side dependency in src-tauri/Cargo.toml)
 * — no extra npm package needed.
 *
 * Spec §13: native file picker, multi-select, video/audio/image filters.
 */
export async function pickMediaPathsViaTauriDialog(): Promise<string[]> {
  const w = window as unknown as {
    __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
  };
  if (!w.__TAURI_INTERNALS__?.invoke) {
    throw new Error("Tauri dialog plugin is not available in this build.");
  }
  try {
    const result = await w.__TAURI_INTERNALS__.invoke("plugin:dialog|open", {
      multiple: true,
      directory: false,
      filters: [{ name: "Media", extensions: MEDIA_EXTS }],
    });
    // Tauri 2 dialog plugin returns `string | string[] | null` depending on `multiple`.
    if (result == null) return [];
    if (Array.isArray(result)) return result.filter((p): p is string => typeof p === "string");
    if (typeof result === "string") return [result];
    return [];
  } catch (e) {
    // The plugin isn't installed or the user dismissed the dialog. Degrade
    // gracefully: return empty so the UI doesn't crash.
    console.warn("Tauri dialog failed:", e);
    return [];
  }
}

// ---- typed commands ----

export const api = {
  invoke,
  projectSnapshot: () => invoke<ProjectSnapshot>("project_snapshot"),
  importMedia: (paths: string[]) => invoke<{ imported: number; skipped: { path: string; reason: string }[] }>("import_media", { paths }),
  applyAiRequest: (request: string) => invoke<AiDiff>("ai_apply_request", { request }),
  undo: () => invoke<string>("undo"),
  redo: () => invoke<string>("redo"),
  save: () => invoke<void>("save"),
  proxyPath: (sourceId: string) => invoke<{ path: string }>("proxy_path", { sourceId }),
  clearCache: () => invoke<void>("clear_cache"),
  cacheUsage: () => invoke<{ bytes: number; limitBytes: number }>("cache_usage"),
  doctor: () =>
    invoke<{
      appVersion: string;
      runtime: string;
      ffmpeg: { state: string; version?: string; minimum: string; path?: string; message: string; usingBundled: boolean };
    }>("doctor"),

  // keys + provider
  setApiKey: (key: string) => invoke<{ ok: boolean; backend: string }>("set_api_key", { key }),
  clearApiKey: () => invoke<{ ok: boolean; backend: string }>("clear_api_key"),
  keyState: () => invoke<{ stored: boolean; backend: string }>("key_state"),
  getSettings: () => invoke<AppSettingsInfo>("get_settings"),
  setSetting: (key: string, value: string | boolean) => invoke<void>("set_setting", { key, value }),
  testConnection: () => invoke<ConnectionResult>("ai_test_connection"),
  listModels: (refresh: boolean) =>
    invoke<{ models: ModelInfo[]; cached: boolean; count: number; warning?: string }>(
      "ai_list_models",
      { refresh },
    ),
  testModel: () => invoke<{ ok: boolean; model: string; state: string }>("ai_test_model"),

  // export
  exportPresets: () => invoke<{ presets: ExportPresetInfo[] }>("export_presets"),
  setExportSettings: (s: Record<string, unknown>) =>
    invoke<{ ok: boolean }>("set_export_settings", s),
  exportStart: () => invoke<{ ok: boolean; path: string }>("export_start"),
  exportStatus: () => invoke<ExportJobStatus>("export_status"),
  exportCancel: () => invoke<{ ok: boolean }>("export_cancel"),
  renderFinal: () => invoke<{ path: string; seconds: number }>("render_final"),

  // catalogs
  effectCatalog: () => invoke<{ video: EffectDefInfo[]; audio: EffectDefInfo[] }>("effect_catalog"),
  transitionCatalog: () =>
    invoke<{ kinds: { id: string; label: string }[] }>("transition_catalog"),
  captionStyleCatalog: () =>
    invoke<{ styles: { id: string; label: string }[] }>("caption_style_catalog"),

  // editing
  splitClip: (itemId: string, atMs: number) =>
    invoke<{ ok: boolean }>("split_clip", { itemId, atMs }),
  moveItem: (itemId: string, timelineStartMs: number) =>
    invoke<{ ok: boolean }>("move_item", { itemId, timelineStartMs }),
  deleteItem: (itemId: string) => invoke<{ ok: boolean }>("delete_item", { itemId }),
  duplicateItem: (itemId: string) => invoke<{ ok: boolean }>("duplicate_item", { itemId }),
  trimClip: (itemId: string, sourceInMs: number, sourceOutMs: number, timelineStartMs: number) =>
    invoke<{ ok: boolean }>("trim_clip", { itemId, sourceInMs, sourceOutMs, timelineStartMs }),
  addEffect: (itemId: string, defId: string, params?: Record<string, number>) =>
    invoke<{ ok: boolean; effectId: string }>("add_effect", { itemId, defId, params }),
  removeEffect: (itemId: string, effectId: string) =>
    invoke<{ ok: boolean }>("remove_effect", { itemId, effectId }),
  setEffectParam: (itemId: string, effectId: string, name: string, value: number) =>
    invoke<{ ok: boolean }>("set_effect_param", { itemId, effectId, name, value }),
  setItemVolume: (itemId: string, volume: number) =>
    invoke<{ ok: boolean }>("set_item_volume", { itemId, volume }),
  setItemOpacity: (itemId: string, opacity: number) =>
    invoke<{ ok: boolean }>("set_item_opacity", { itemId, opacity }),
  setClipSpeed: (itemId: string, speed: number) =>
    invoke<{ ok: boolean }>("set_clip_speed", { itemId, speed }),
  addText: (text: string, opts: { startMs: number; durationMs: number; position: string; kind?: string; scale?: number }) =>
    invoke<{ ok: boolean }>("add_text", { text, ...opts }),
  updateText: (itemId: string, patch: Record<string, unknown>) =>
    invoke<{ ok: boolean }>("update_text", { itemId, ...patch }),
  setTransitions: (transitions: { afterIndex: number; kind: string; durationMs: number }[]) =>
    invoke<{ ok: boolean }>("set_transitions", { transitions }),
  setColor: (patch: Record<string, number>) => invoke<{ ok: boolean }>("set_color", patch),
  setReframe: (enabled: boolean, ratio?: string, mode?: string) =>
    invoke<{ ok: boolean }>("set_reframe", { enabled, ratio, mode }),
  setAudioMaster: (patch: Record<string, unknown>) =>
    invoke<{ ok: boolean }>("set_audio_master", patch),
  setTrackProps: (trackId: string, patch: { muted?: boolean; solo?: boolean; locked?: boolean }) =>
    invoke<{ ok: boolean }>("set_track_props", { trackId, ...patch }),
  addTrack: (kind: string, name: string) => invoke<{ ok: boolean }>("add_track", { kind, name }),
  removeTrack: (trackId: string) => invoke<{ ok: boolean }>("remove_track", { trackId }),
  setCaptions: (
    entries: { startMs: number; endMs: number; text: string }[],
    style: string,
  ) => invoke<{ ok: boolean }>("set_captions", { entries, style }),
  removeCaptions: () => invoke<{ ok: boolean }>("remove_captions"),
  removeSource: (sourceId: string) => invoke<{ ok: boolean }>("remove_source", { sourceId }),
  addSourceToTimeline: (sourceId: string, startMs: number) =>
    invoke<{ ok: boolean }>("add_source_to_timeline", { sourceId, startMs }),
  revealSource: (sourceId: string) => invoke<{ ok: boolean }>("reveal_source", { sourceId }),
};

/** Convert an app path to something a <video>/<img> element can load. */
export function assetUrl(p: string): string {
  if (backendKind() === "tauri") {
    const w = window as unknown as { __TAURI_INTERNALS__?: { convertFileSrc?: (p: string) => string } };
    if (w.__TAURI_INTERNALS__?.convertFileSrc) {
      return w.__TAURI_INTERNALS__.convertFileSrc(p);
    }
  }
  if (backendKind() === "server") {
    // Jailed server-side: project dir, cache dir, export dir only.
    return `media?path=${encodeURIComponent(p)}`;
  }
  return p;
}
