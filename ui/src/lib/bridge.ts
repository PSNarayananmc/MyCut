/**
 * Typed bridge to the Rust core. In the Tauri app this maps to `invoke`;
 * in a plain browser (dev) the backend is absent and every call resolves
 * to a HONEST "backend unavailable" state — nothing is faked.
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

export interface TimelineItem {
  id: string;
  kind: string;
  timeline_start_ms: number;
  timeline_duration_ms: number;
  label: string;
}

export interface TrackInfo {
  id: string;
  kind: string;
  name: string;
  items: TimelineItem[];
}

export interface ProjectSnapshot {
  name: string;
  sources: SourceInfo[];
  tracks: TrackInfo[];
  duration_ms: number;
  color_set: boolean;
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

type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** Real Tauri IPC when available; undefined in a plain browser. */
function tauriInvoke(): InvokeFn | null {
  const w = window as unknown as { __TAURI_INTERNALS__?: { invoke: InvokeFn } };
  if (w.__TAURI_INTERNALS__?.invoke) {
    return w.__TAURI_INTERNALS__.invoke;
  }
  return null;
}

export const backendAvailable = (): boolean => tauriInvoke() !== null;

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const fn = tauriInvoke();
  if (!fn) {
    throw new Error("backend-unavailable");
  }
  return fn(cmd, args) as Promise<T>;
}

// ---- typed commands ----

export const api = {
  invoke,
  projectSnapshot: () => invoke<ProjectSnapshot>("project_snapshot"),
  importMedia: (paths: string[]) => invoke<{ sources: SourceInfo[] }>("import_media", { paths }),
  applyAiRequest: (request: string) => invoke<AiDiff>("ai_apply_request", { request }),
  renderFinal: (outPath: string) => invoke<{ seconds: number }>("render_final", { outPath }),
  renderPreviewRange: (startMs: number, endMs: number) =>
    invoke<{ path: string }>("render_preview_range", { startMs, endMs }),
  undo: () => invoke<string>("undo"),
  redo: () => invoke<string>("redo"),
  save: () => invoke<void>("save"),
  testConnection: () => invoke<{ ok: boolean; state: string }>("ai_test_connection"),
  setApiKey: (key: string) => invoke<void>("set_api_key", { key }),
  getSettings: () =>
    invoke<{ baseUrl: string; model: string; framesEnabled: boolean; profile: string }>("get_settings"),
  setSetting: (key: string, value: string | boolean) => invoke<void>("set_setting", { key, value }),
  clearCache: () => invoke<void>("clear_cache"),
  cacheUsage: () => invoke<{ bytes: number; limitBytes: number }>("cache_usage"),
  proxyPath: (sourceId: string) => invoke<{ path: string }>("proxy_path", { sourceId }),
};

/** Convert an app path to something a webview <video> can load. */
export function assetUrl(p: string): string {
  const w = window as unknown as { __TAURI_INTERNALS__?: { convertFileSrc?: (p: string) => string } };
  if (w.__TAURI_INTERNALS__?.convertFileSrc) {
    return w.__TAURI_INTERNALS__.convertFileSrc(p);
  }
  return p;
}
