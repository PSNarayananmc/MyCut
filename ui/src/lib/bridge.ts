/**
 * Typed bridge to the Rust core. Two transports, one command surface:
 * - Tauri app      → `invoke` over Tauri IPC (`__TAURI_INTERNALS__`)
 * - Local web run  → `POST /api/<cmd>` JSON to the loopback server
 *                    (`mycut` binary — the Ubuntu 20.04 runtime, D15)
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

// ---- typed commands ----

export const api = {
  invoke,
  projectSnapshot: () => invoke<ProjectSnapshot>("project_snapshot"),
  importMedia: (paths: string[]) => invoke<{ ok: boolean }>("import_media", { paths }),
  applyAiRequest: (request: string) => invoke<AiDiff>("ai_apply_request", { request }),
  renderFinal: (outPath: string) => invoke<{ path: string; seconds: number }>("render_final", { outPath }),
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
  doctor: () =>
    invoke<{
      appVersion: string;
      runtime: string;
      ffmpeg: { state: string; version?: string; minimum: string; path?: string; message: string; usingBundled: boolean };
    }>("doctor"),
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
