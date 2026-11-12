// Where a workspace's files live: kept in this browser, or in a folder on this computer. Either way a
// workspace is files at paths like `nodes/<id>.yaml`, and the engine says which to write.

import { readFile, readFiles, writeFiles, type Files } from "./db";

export interface FileInfo {
  path: string;
  size: number;
}

export interface FileText extends FileInfo {
  text: string | null;
  truncated: boolean;
}

export interface Writes {
  write: Files;
  delete: string[];
}

/** When a file last changed, as far as can be told without reading it. */
export type Stamp = { size: number; modified: number };

export interface FileStore {
  readonly kind: "browser" | "folder";
  /** The files the engine reads: settings, nodes and proposals. */
  readWorkspace(): Promise<Files>;
  /** Everything there, for the Files tab. */
  list(): Promise<FileInfo[]>;
  read(path: string): Promise<FileText>;
  /** Write and remove these. */
  commit(w: Writes): Promise<void>;
}

/** The files that make up a workspace, as the engine reads them. Anything else is the user's own. */
export function isWorkspaceFile(path: string): boolean {
  if (path === "recollect.toml" || path === "sample-format") return true;
  const m = path.match(/^(nodes|proposals)\/([^/]+)$/);
  if (!m) return false;
  const ext = m[1] === "nodes" ? ".yaml" : ".json";
  return !m[2].startsWith(".") && m[2].endsWith(ext) && m[2].length > ext.length;
}

const MOST = 2 * 1024 * 1024;
const bytes = (s: string) => new TextEncoder().encode(s).length;

/** A workspace kept in this browser, in IndexedDB. Each change lands whole or not at all. */
export class BrowserStore implements FileStore {
  readonly kind = "browser";
  constructor(readonly id: string) {}

  async readWorkspace(): Promise<Files> {
    const all = await readFiles(this.id);
    return Object.fromEntries(Object.entries(all).filter(([p]) => isWorkspaceFile(p)));
  }

  async list(): Promise<FileInfo[]> {
    const all = await readFiles(this.id);
    return Object.entries(all)
      .map(([path, text]) => ({ path, size: bytes(text) }))
      .sort((a, b) => a.path.localeCompare(b.path));
  }

  async read(path: string): Promise<FileText> {
    const text = await readFile(this.id, path);
    if (text === null) throw Object.assign(new Error(`not found: file ${path}`), { kind: "not_found" });
    return { path, size: bytes(text), text, truncated: false };
  }

  commit(w: Writes): Promise<void> {
    return writeFiles(this.id, w.write, w.delete);
  }
}

async function dirAt(root: FileSystemDirectoryHandle, parts: string[], create: boolean): Promise<FileSystemDirectoryHandle | null> {
  let d = root;
  for (const p of parts) {
    try {
      d = await d.getDirectoryHandle(p, { create });
    } catch (e) {
      if (!create && (e as DOMException).name === "NotFoundError") return null;
      throw e;
    }
  }
  return d;
}

function split(path: string): [string[], string] {
  const parts = path.split("/");
  return [parts.slice(0, -1), parts[parts.length - 1]];
}

/**
 * A folder on this computer, through the File System Access API. Each file is replaced whole when
 * it's written, so a reader never sees half of one. Other programs can change the files too, so
 * `scan` says when each last changed without reading it, for noticing that.
 */
export class FolderStore implements FileStore {
  readonly kind = "folder";
  constructor(readonly root: FileSystemDirectoryHandle) {}

  private async file(path: string): Promise<File | null> {
    const [dirs, name] = split(path);
    const d = await dirAt(this.root, dirs, false);
    if (!d) return null;
    try {
      return await (await d.getFileHandle(name)).getFile();
    } catch (e) {
      if ((e as DOMException).name === "NotFoundError" || (e as DOMException).name === "TypeMismatchError") return null;
      throw e;
    }
  }

  /** When each workspace file last changed. */
  async scan(): Promise<Map<string, Stamp>> {
    const out = new Map<string, Stamp>();
    const visit = async (d: FileSystemDirectoryHandle | null, prefix: string) => {
      if (!d) return;
      for await (const entry of d.values()) {
        const path = prefix + entry.name;
        if (entry.kind === "file" && isWorkspaceFile(path)) {
          const f = await (entry as FileSystemFileHandle).getFile();
          out.set(path, { size: f.size, modified: f.lastModified });
        }
      }
    };
    await visit(this.root, "");
    await visit(await dirAt(this.root, ["nodes"], false), "nodes/");
    await visit(await dirAt(this.root, ["proposals"], false), "proposals/");
    return out;
  }

  async readText(path: string): Promise<string | null> {
    const f = await this.file(path);
    return f ? f.text() : null;
  }

  async readWorkspace(): Promise<Files> {
    const out: Files = {};
    for (const path of (await this.scan()).keys()) {
      const text = await this.readText(path);
      if (text !== null) out[path] = text;
    }
    return out;
  }

  async list(): Promise<FileInfo[]> {
    const out: FileInfo[] = [];
    const walk = async (d: FileSystemDirectoryHandle, prefix: string) => {
      for await (const entry of d.values()) {
        if (entry.kind === "directory") {
          if (entry.name !== ".git") await walk(entry as FileSystemDirectoryHandle, `${prefix}${entry.name}/`);
        } else if (!(entry.name.startsWith(".") && (entry.name.endsWith(".crswap") || entry.name.endsWith(".tmp"))) && entry.name !== ".recollect.lock") {
          out.push({ path: prefix + entry.name, size: (await (entry as FileSystemFileHandle).getFile()).size });
        }
      }
    };
    await walk(this.root, "");
    return out.sort((a, b) => a.path.localeCompare(b.path));
  }

  async read(path: string): Promise<FileText> {
    const listed = (await this.list()).some((f) => f.path === path);
    const f = listed ? await this.file(path) : null;
    if (!f) throw Object.assign(new Error(`not found: file ${path}`), { kind: "not_found" });
    const buf = new Uint8Array(await f.slice(0, MOST).arrayBuffer());
    const truncated = f.size > MOST;
    let text: string | null;
    try {
      text = buf.includes(0) ? null : new TextDecoder("utf-8", { fatal: !truncated }).decode(buf);
    } catch {
      text = null;
    }
    return { path, size: f.size, text, truncated };
  }

  async commit(w: Writes): Promise<void> {
    for (const [path, text] of Object.entries(w.write)) {
      const [dirs, name] = split(path);
      const d = (await dirAt(this.root, dirs, true))!;
      const out = await (await d.getFileHandle(name, { create: true })).createWritable();
      await out.write(text);
      await out.close();
    }
    for (const path of w.delete) {
      const [dirs, name] = split(path);
      const d = await dirAt(this.root, dirs, false);
      try {
        await d?.removeEntry(name);
      } catch (e) {
        if ((e as DOMException).name !== "NotFoundError") throw e;
      }
    }
  }
}
