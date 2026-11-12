// What the page and the engine's worker say to each other.

import type { Files } from "./db";

export type ErrorKind = "nothing_open" | "not_found" | "refused" | "other";

export interface EngineError {
  kind: ErrorKind;
  message: string;
}

/** Where a change stands: written, still being written, or not written because something's wrong. */
export interface SaveStatus {
  state: "saved" | "saving" | "failed";
  error?: string;
  /** Another tab has this workspace open, so this one only looks. */
  readOnly?: boolean;
}

export type Request =
  | { op: "workspaces" }
  | { op: "start" }
  | { op: "openSample"; name: string; reset: boolean }
  | { op: "createBrowser"; name: string; systems: string[] }
  | { op: "importBrowser"; name: string; files: Files }
  | { op: "openBrowser"; id: string }
  | { op: "openFolder"; handle: FileSystemDirectoryHandle; id?: string; create?: { name: string; systems: string[] } }
  | { op: "forget"; id: string }
  | { op: "call"; method: string; args: unknown }
  | { op: "files" }
  | { op: "file"; path: string }
  | { op: "export" }
  | { op: "visible"; visible: boolean }
  | { op: "sync" };

/** `seq` pairs an answer with its question. It isn't `id`, which some requests have of their own. */
export type ToWorker = { seq: number; request: Request };

export type ServerEvent = { type: "graph_changed"; by: string } | { type: "proposals_changed" } | { type: "workspace_changed" } | { type: "resync" };

export type FromWorker = { seq: number; ok: true; value: unknown } | { seq: number; ok: false; error: EngineError } | { event: ServerEvent } | { status: SaveStatus };
