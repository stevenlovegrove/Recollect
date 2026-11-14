// Where the app's answers come from. Served by `recollect serve`, it asks the server, which also
// serves Claude over MCP. Anywhere else, such as a static site, the engine runs here, in a worker,
// and keeps workspaces in this browser or in folders on this computer.

import type { FileInfo, FileText, Workspaces } from "./api";
import type { ServerEvent } from "./engine/protocol";

export type { ServerEvent };

/** Whether what's on screen is safe: saved, or connected to the server that saves it. */
export interface Status {
  state: "live" | "offline" | "saved" | "saving" | "failed";
  error?: string;
  /** Another tab has this workspace open, so this one only looks. */
  readOnly?: boolean;
}

export class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

export interface Backend {
  readonly kind: "http" | "local";
  /** Anything the engine answers, by name: see recollect_core::dispatch. */
  call<T>(method: string, args?: unknown): Promise<T>;
  workspaces(): Promise<Workspaces>;
  openSample(name: string, reset?: boolean): Promise<void>;
  files(): Promise<FileInfo[]>;
  file(path: string): Promise<FileText>;
  subscribe(onEvent: (e: ServerEvent) => void, onStatus: (s: Status) => void): () => void;

  // Served by `recollect serve`: workspaces are directories it can reach by path.
  openPath?(path: string): Promise<void>;
  createAt?(path: string, name: string): Promise<void>;

  // Run here: workspaces are kept in this browser, or are folders this browser has been allowed into.
  /** Whether this browser can open folders at all. */
  readonly folders?: boolean;
  createInBrowser?(name: string): Promise<void>;
  openFolder?(handle?: FileSystemDirectoryHandle): Promise<void>;
  createInFolder?(name: string, handle?: FileSystemDirectoryHandle): Promise<void>;
  openKnown?(id: string): Promise<void>;
  forget?(id: string): Promise<void>;
  exportZip?(): Promise<void>;
  importZip?(file: File): Promise<void>;
  importFolder?(files: FileList): Promise<void>;
}

const CODES: Record<string, number> = { nothing_open: 409, not_found: 404, refused: 422, other: 500 };
export const statusOf = (kind: string) => CODES[kind] ?? 500;

class HttpBackend implements Backend {
  readonly kind = "http";

  private async req<T>(path: string, init?: RequestInit): Promise<T> {
    const r = await fetch(path, { headers: { "Content-Type": "application/json" }, ...init });
    if (!r.ok) {
      let msg = `${r.status} ${r.statusText}`;
      try {
        msg = (await r.json()).error ?? msg;
      } catch {
        /* not JSON */
      }
      throw new ApiError(msg, r.status);
    }
    return r.status === 204 ? (undefined as T) : r.json();
  }

  private post<T>(path: string, body: unknown) {
    return this.req<T>(path, { method: "POST", body: JSON.stringify(body) });
  }

  call<T>(method: string, args: unknown = {}) {
    return this.post<T>(`/api/call/${method}`, args);
  }
  workspaces() {
    return this.req<Workspaces>("/api/workspaces");
  }
  openSample(sample: string, reset = false) {
    return this.post<void>("/api/workspaces/open", { sample, reset });
  }
  openPath(path: string) {
    return this.post<void>("/api/workspaces/open", { path });
  }
  createAt(path: string, name: string) {
    return this.post<void>("/api/workspaces/new", { path, name });
  }
  files() {
    return this.req<FileInfo[]>("/api/files");
  }
  file(path: string) {
    return this.req<FileText>(`/api/file?path=${encodeURIComponent(path)}`);
  }

  /** Server events, reconnecting with backoff. */
  subscribe(onEvent: (e: ServerEvent) => void, onStatus: (s: Status) => void) {
    let ws: WebSocket | null = null;
    let closed = false;
    let delay = 500;
    const connect = () => {
      const proto = location.protocol === "https:" ? "wss" : "ws";
      ws = new WebSocket(`${proto}://${location.host}/api/events`);
      ws.onopen = () => {
        delay = 500;
        onStatus({ state: "live" });
        onEvent({ type: "resync" });
      };
      ws.onmessage = (m) => onEvent(JSON.parse(m.data));
      ws.onclose = () => {
        onStatus({ state: "offline" });
        if (!closed) setTimeout(connect, (delay = Math.min(delay * 2, 8000)));
      };
    };
    connect();
    return () => {
      closed = true;
      ws?.close();
    };
  }
}

/** Which backend this page uses: the server that served it says so, and `?backend=` overrides it. */
function chosen(): "http" | "local" {
  const asked = new URLSearchParams(location.search).get("backend");
  if (asked === "http" || asked === "local") return asked;
  return document.querySelector('meta[name="recollect-backend"]')?.getAttribute("content") === "http" ? "http" : "local";
}

let made: Promise<Backend> | null = null;

/** The backend, made the first time it's asked for. It's `window.recollect` too, for debugging. */
export function backend(): Promise<Backend> {
  made ??= (chosen() === "http" ? Promise.resolve<Backend>(new HttpBackend()) : import("./engine/client").then((m) => new m.LocalBackend())).then((b) => {
    (window as unknown as { recollect: Backend }).recollect = b;
    return b;
  });
  return made;
}
