// The page's side of the engine's worker. Picking a folder and allowing the page into it have to
// happen here, from a click, so this does those and hands the folder to the worker.

import { strFromU8, strToU8, unzipSync, zipSync } from "fflate";
import type { FileInfo, FileText, Workspaces } from "../api";
import { ApiError, statusOf, type Backend, type Status } from "../backend";
import { getKnown, type Files } from "./db";
import type { FromWorker, Request, ServerEvent, ToWorker } from "./protocol";

export class LocalBackend implements Backend {
  readonly kind = "local";
  readonly folders = typeof window.showDirectoryPicker === "function";
  private worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  private next = 0;
  private waiting = new Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void }>();
  private onEvent: ((e: ServerEvent) => void)[] = [];
  private onStatus: ((s: Status) => void)[] = [];
  private status: Status = { state: "saved" };
  private started: Promise<unknown>;

  constructor() {
    this.worker.onmessage = (m: MessageEvent<FromWorker>) => {
      const d = m.data;
      if ("event" in d) this.onEvent.forEach((f) => f(d.event));
      else if ("status" in d) {
        this.status = d.status;
        this.onStatus.forEach((f) => f(d.status));
      } else {
        const w = this.waiting.get(d.seq);
        this.waiting.delete(d.seq);
        if (d.ok) w?.resolve(d.value);
        else w?.reject(new ApiError(d.error.message, statusOf(d.error.kind)));
      }
    };
    this.worker.onerror = (e) => console.error("the engine stopped:", e.message);
    // Whatever was open last, if it can be opened without asking.
    this.started = this.ask({ op: "start" }).catch((e) => console.warn("couldn't reopen what was open last:", e));
    const showing = () => {
      this.ask({ op: "visible", visible: !document.hidden });
      if (!document.hidden) this.ask({ op: "sync" }).catch(() => {});
    };
    document.addEventListener("visibilitychange", showing);
    window.addEventListener("focus", () => this.ask({ op: "sync" }).catch(() => {}));
    // Leaving with changes not yet written would lose them.
    window.addEventListener("beforeunload", (e) => {
      if (this.status.state === "saving" || this.status.state === "failed") e.preventDefault();
    });
  }

  private ask<T = unknown>(request: Request): Promise<T> {
    const seq = ++this.next;
    return new Promise<T>((resolve, reject) => {
      this.waiting.set(seq, { resolve: resolve as (v: unknown) => void, reject });
      this.worker.postMessage({ seq, request } satisfies ToWorker);
    });
  }

  async call<T>(method: string, args: unknown = {}) {
    await this.started;
    return this.ask<T>({ op: "call", method, args });
  }
  async workspaces() {
    await this.started;
    return this.ask<Workspaces>({ op: "workspaces" });
  }
  openSample(name: string, reset = false) {
    return this.ask<void>({ op: "openSample", name, reset });
  }
  files() {
    return this.ask<FileInfo[]>({ op: "files" });
  }
  file(path: string) {
    return this.ask<FileText>({ op: "file", path });
  }
  createInBrowser(name: string) {
    return this.ask<void>({ op: "createBrowser", name });
  }

  private async pick(): Promise<FileSystemDirectoryHandle> {
    if (!window.showDirectoryPicker) throw new ApiError("This browser can't open folders. Chrome and Edge can, or keep the workspace in this browser instead.", 422);
    return window.showDirectoryPicker({ id: "recollect", mode: "readwrite" });
  }

  /** Open a folder: one picked now, or one already in hand. */
  async openFolder(handle?: FileSystemDirectoryHandle) {
    handle ??= await this.pick();
    await this.ask({ op: "openFolder", handle });
  }
  async createInFolder(name: string, handle?: FileSystemDirectoryHandle) {
    handle ??= await this.pick();
    await this.ask({ op: "openFolder", handle, create: { name } });
  }

  /** Open one this browser knows. A folder may need allowing again, which has to come from a click. */
  async openKnown(id: string) {
    const k = await getKnown(id);
    if (!k) throw new ApiError("This browser doesn't know that workspace any more.", 404);
    if (k.kind === "browser") return this.ask<void>({ op: "openBrowser", id });
    const handle = k.handle!;
    let allowed = await handle.queryPermission?.({ mode: "readwrite" });
    if (allowed !== "granted") allowed = await handle.requestPermission?.({ mode: "readwrite" });
    if (allowed !== "granted") throw new ApiError(`Recollect wasn't allowed into ${handle.name}.`, 422);
    await this.ask({ op: "openFolder", handle, id });
  }
  forget(id: string) {
    return this.ask<void>({ op: "forget", id });
  }

  /** Save the open workspace's files as a zip, laid out as a folder workspace is. */
  async exportZip() {
    const { name, files } = await this.ask<{ name: string; files: Files }>({ op: "export" });
    const zipped = zipSync(Object.fromEntries(Object.entries(files).map(([p, t]) => [p, strToU8(t)])));
    const url = URL.createObjectURL(new Blob([zipped as BlobPart], { type: "application/zip" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `${name.replace(/[^\w.-]+/g, "-") || "workspace"}.zip`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  /** Keep a zipped workspace in this browser. Its files can sit in a folder inside the zip. */
  async importZip(file: File) {
    const entries = unzipSync(new Uint8Array(await file.arrayBuffer()));
    const texts: Files = {};
    for (const [p, data] of Object.entries(entries)) if (!p.endsWith("/")) texts[p] = strFromU8(data);
    await this.importFiles(texts, file.name.replace(/\.zip$/i, ""));
  }

  /** Keep a copy of a workspace folder in this browser, for browsers that can't open folders. */
  async importFolder(list: FileList) {
    const texts: Files = {};
    for (const f of Array.from(list)) texts[f.webkitRelativePath || f.name] = await f.text();
    await this.importFiles(texts, list[0]?.webkitRelativePath.split("/")[0] ?? "Imported");
  }

  private async importFiles(texts: Files, fallback: string) {
    // Wherever recollect.toml is, that's the workspace's root.
    const config = Object.keys(texts)
      .filter((p) => p === "recollect.toml" || p.endsWith("/recollect.toml"))
      .sort((a, b) => a.length - b.length)[0];
    if (!config) throw new ApiError("That isn't a workspace: there's no recollect.toml in it.", 422);
    const root = config.slice(0, -"recollect.toml".length);
    const files: Files = {};
    for (const [p, t] of Object.entries(texts)) if (p.startsWith(root)) files[p.slice(root.length)] = t;
    const name = files["recollect.toml"].match(/^name\s*=\s*"((?:[^"\\]|\\.)*)"/m)?.[1] ?? fallback;
    await this.ask({ op: "importBrowser", name, files });
  }

  subscribe(onEvent: (e: ServerEvent) => void, onStatus: (s: Status) => void) {
    this.onEvent.push(onEvent);
    this.onStatus.push(onStatus);
    onStatus(this.status);
    this.started.then(() => onEvent({ type: "resync" }));
    return () => {
      this.onEvent = this.onEvent.filter((f) => f !== onEvent);
      this.onStatus = this.onStatus.filter((f) => f !== onStatus);
    };
  }
}
