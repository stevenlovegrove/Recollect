/// <reference lib="webworker" />
// The engine, and the one workspace open in this tab. It reads the workspace's files from wherever
// they live, hands them to the engine, writes what each change says to, and notices when the files
// change underneath it: in another tab, or, for a folder, in another program such as an editor or
// Claude working through `recollect serve`.

import init, { Engine, catalogue, newFiles, sampleFiles, sampleFormat } from "./pkg/recollect_wasm.js";
import { forgetKnown, getKnown, getMeta, known, putKnown, readFile, removeAllFiles, setMeta, writeFiles, type Files, type Known } from "./db";
import type { EngineError, ErrorKind, FromWorker, Request, SaveStatus, ServerEvent, ToWorker } from "./protocol";
import { BrowserStore, FolderStore, isWorkspaceFile, type FileStore, type Stamp, type Writes } from "./stores";

declare const self: DedicatedWorkerGlobalScope;

const ready = init();
let engine: Engine;

interface Current {
  known: Known;
  store: FileStore;
  /** Another tab has it, so this one only looks, until that one lets go. */
  readOnly: boolean;
  release: () => void;
  stopWaiting?: () => void;
  observer?: FileSystemObserver;
  /** For a folder: when each file last changed, as of the last look. */
  stamps: Map<string, Stamp>;
}

let current: Current | null = null;
let visible = true;
let saving = 0;
let failed: string | undefined;
/** What couldn't be written, to try again with the next change. */
let unsaved: Writes = { write: {}, delete: [] };
const channel = new BroadcastChannel("recollect");

/** These change the workspace. */
const CHANGES = new Set(["apply", "undo", "redo", "propose", "accept", "reject"]);

class Failure extends Error {
  constructor(
    readonly kind: ErrorKind,
    message: string,
  ) {
    super(message);
  }
}

const post = (m: FromWorker) => self.postMessage(m);
const emit = (event: ServerEvent) => post({ event });

function status() {
  const s: SaveStatus = { state: failed ? "failed" : saving ? "saving" : "saved", error: failed, readOnly: current?.readOnly };
  post({ status: s });
}

/** What went wrong, from the engine (which says it as JSON), a store, or anything else. */
function errorOf(e: unknown): EngineError {
  if (e instanceof Failure) return { kind: e.kind, message: e.message };
  if (typeof e === "string") {
    try {
      const parsed = JSON.parse(e) as EngineError;
      if (parsed.kind && parsed.message) return parsed;
    } catch {
      /* not from the engine */
    }
    return { kind: "other", message: e };
  }
  const kind = (e as { kind?: ErrorKind })?.kind;
  return { kind: kind ?? "other", message: e instanceof Error ? e.message : String(e) };
}

const empty = (w: Writes) => !Object.keys(w.write).length && !w.delete.length;

function merge(a: Writes, b: Writes): Writes {
  const write = { ...a.write };
  const del = new Set(a.delete);
  for (const p of b.delete) {
    delete write[p];
    del.add(p);
  }
  for (const [p, t] of Object.entries(b.write)) {
    write[p] = t;
    del.delete(p);
  }
  return { write, delete: [...del] };
}

/** Write what a change says to. A failure is kept, shown, and tried again with the next one. */
async function persist(writes: Writes) {
  const c = current;
  if (!c || c.readOnly) return;
  const all = merge(unsaved, writes);
  if (empty(all)) return;
  saving++;
  status();
  try {
    await c.store.commit(all);
    unsaved = { write: {}, delete: [] };
    failed = undefined;
    if (c.store instanceof FolderStore) {
      for (const p of all.delete) c.stamps.delete(p);
      const now = await c.store.scan();
      for (const p of Object.keys(all.write)) {
        const s = now.get(p);
        if (s) c.stamps.set(p, s);
      }
    }
    channel.postMessage({ changed: c.known.id });
  } catch (e) {
    unsaved = all;
    failed = `couldn't save: ${errorOf(e).message}`;
  } finally {
    saving--;
    status();
  }
}

// ---------- one writer per workspace ----------

/** Hold the workspace for this tab, if no other tab has it. */
function takeLock(id: string): Promise<(() => void) | null> {
  if (!navigator.locks) return Promise.resolve(() => {});
  return new Promise((resolve) => {
    navigator.locks
      .request(`recollect:${id}`, { ifAvailable: true }, (lock) => {
        if (!lock) {
          resolve(null);
          return undefined;
        }
        return new Promise<void>((release) => resolve(() => release()));
      })
      .catch(() => resolve(null));
  });
}

/** Wait for the tab that has it to let go, and take it then. */
function awaitLock(id: string, granted: (release: () => void) => void): () => void {
  const ac = new AbortController();
  navigator.locks?.request(`recollect:${id}`, { signal: ac.signal }, () => new Promise<void>((release) => granted(release))).catch(() => {});
  return () => ac.abort();
}

// ---------- opening ----------

function close() {
  if (!current) return;
  current.release();
  current.stopWaiting?.();
  current.observer?.disconnect();
  current = null;
  engine.close();
  unsaved = { write: {}, delete: [] };
  failed = undefined;
}

async function open(k: Known, store: FileStore) {
  close();
  const release = await takeLock(k.id);
  const files = await store.readWorkspace();
  const opened = JSON.parse(engine.open(k.id, k.location, k.sample, JSON.stringify(files))) as { problems: string[]; writes: Writes; name: string };
  for (const p of opened.problems) console.info(`${k.location}: ${p}`);
  const c: Current = { known: { ...k, name: opened.name, opened: Date.now() }, store, readOnly: !release, release: release ?? (() => {}), stamps: new Map() };
  current = c;
  if (store instanceof FolderStore) {
    c.stamps = await store.scan();
    watch(c, store);
  }
  if (c.readOnly) {
    c.stopWaiting = awaitLock(k.id, (rel) => {
      if (current !== c) return rel();
      c.readOnly = false;
      c.release = rel;
      c.stopWaiting = undefined;
      status();
      serial(() => sync());
    });
  }
  await persist(opened.writes);
  await putKnown(c.known);
  await setMeta("last", k.id);
  status();
  emit({ type: "workspace_changed" });
}

/** For a folder: notice when other programs change it. */
function watch(c: Current, store: FolderStore) {
  if (typeof FileSystemObserver === "undefined") return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const observer = new FileSystemObserver(() => {
      clearTimeout(timer);
      // Debounce bursts: editors, git checkouts, and this tab's own writes.
      timer = setTimeout(() => current === c && serial(() => sync()), 150);
    });
    observer.observe(store.root, { recursive: true }).catch(() => {});
    c.observer = observer;
  } catch {
    /* polling will do */
  }
}

/** Read the files again, and bring in whatever someone else changed. */
async function sync() {
  const c = current;
  if (!c) return;
  const changes: Record<string, string | null> = {};
  const mine = JSON.parse(engine.files()) as Files;
  if (c.store instanceof FolderStore) {
    const now = await c.store.scan();
    for (const [p, s] of now) {
      const was = c.stamps.get(p);
      if (!was || was.size !== s.size || was.modified !== s.modified) changes[p] = await c.store.readText(p);
    }
    for (const p of c.stamps.keys()) if (!now.has(p)) changes[p] = null;
    for (const p of Object.keys(mine)) if (isWorkspaceFile(p) && !now.has(p) && !(p in changes) && !(p in unsaved.write)) changes[p] = null;
    c.stamps = now;
  } else {
    const disk = await c.store.readWorkspace();
    for (const p of new Set([...Object.keys(disk), ...Object.keys(mine)])) if (disk[p] !== mine[p] && isWorkspaceFile(p)) changes[p] = disk[p] ?? null;
  }
  if (current !== c || !Object.keys(changes).length) return;
  const r = JSON.parse(engine.filesChanged(JSON.stringify(changes))) as { changed: { graph: boolean; proposals: boolean; workspace: boolean; reopen: boolean; problems: string[] }; writes: Writes };
  for (const p of r.changed.problems) console.warn(p);
  if (r.changed.reopen) {
    await open(c.known, c.store);
    return;
  }
  await persist(r.writes);
  if (r.changed.graph) emit({ type: "graph_changed", by: "disk" });
  if (r.changed.proposals) emit({ type: "proposals_changed" });
  if (r.changed.workspace) emit({ type: "workspace_changed" });
}

const uid = () => crypto.randomUUID();

function sampleTitle(name: string): string {
  const s = (JSON.parse(catalogue()) as { samples: { name: string; title: string }[] }).samples.find((s) => s.name === name);
  if (!s) throw new Failure("not_found", `not found: sample ${name}`);
  return s.title;
}

async function sameFolder(handle: FileSystemDirectoryHandle): Promise<Known | undefined> {
  for (const k of await known()) if (k.kind === "folder" && k.handle && (await k.handle.isSameEntry(handle))) return k;
  return undefined;
}

// ---------- answering ----------

async function handle(req: Request): Promise<unknown> {
  await ready;
  engine ??= new Engine();
  switch (req.op) {
    case "workspaces": {
      const all = await known();
      const { samples, systems } = JSON.parse(catalogue());
      const c = current;
      return {
        current: c ? { name: c.known.name, id: c.known.id, location: c.known.location, sample: c.known.sample ?? null, kind: c.known.kind, readOnly: c.readOnly } : null,
        samples,
        systems,
        recent: all.filter((k) => !k.sample).map(({ handle: _, ...k }) => k),
      };
    }
    case "start": {
      if (current) return null;
      const last = await getMeta<string>("last");
      const k = last ? await getKnown(last) : undefined;
      if (!k) return null;
      if (k.kind === "browser") await open(k, new BrowserStore(k.id));
      else if (k.handle && (await k.handle.queryPermission?.({ mode: "readwrite" })) === "granted") await open(k, new FolderStore(k.handle));
      return null;
    }
    case "openSample": {
      const id = `sample:${req.name}`;
      const title = sampleTitle(req.name);
      if (current?.known.id === id) close();
      // A copy made when samples meant something else would be misread, so it's made again.
      if (req.reset || (await readFile(id, "sample-format")) !== sampleFormat()) {
        await removeAllFiles(id);
        await writeFiles(id, JSON.parse(sampleFiles(req.name)));
      }
      await open({ id, name: title, kind: "browser", location: "this browser", sample: req.name, opened: Date.now() }, new BrowserStore(id));
      return null;
    }
    case "createBrowser":
    case "importBrowser": {
      const files: Files = req.op === "createBrowser" ? JSON.parse(newFiles(req.name, JSON.stringify(req.systems))) : Object.fromEntries(Object.entries(req.files).filter(([p]) => isWorkspaceFile(p)));
      if (!files["recollect.toml"]) throw new Failure("refused", "that isn't a workspace: it has no recollect.toml");
      const id = `browser:${uid()}`;
      await writeFiles(id, files);
      await open({ id, name: req.name, kind: "browser", location: "this browser", opened: Date.now() }, new BrowserStore(id));
      return null;
    }
    case "openBrowser": {
      const k = await getKnown(req.id);
      if (!k || k.kind !== "browser") throw new Failure("not_found", "not found: that workspace isn't in this browser any more");
      await open(k, new BrowserStore(k.id));
      return null;
    }
    case "openFolder": {
      const store = new FolderStore(req.handle);
      const has = (await store.readText("recollect.toml")) !== null;
      if (req.create) {
        if (has) throw new Failure("refused", `${req.handle.name} is already a workspace`);
        await store.commit({ write: JSON.parse(newFiles(req.create.name, JSON.stringify(req.create.systems))), delete: [] });
      } else if (!has) throw new Failure("other", `${req.handle.name} isn't a workspace: it has no recollect.toml`);
      const was = await sameFolder(req.handle);
      const id = was?.id ?? req.id ?? `folder:${uid()}`;
      await open({ id, name: req.create?.name ?? was?.name ?? req.handle.name, kind: "folder", location: req.handle.name, handle: req.handle, opened: Date.now() }, store);
      return null;
    }
    case "forget": {
      const k = await getKnown(req.id);
      if (current?.known.id === req.id) {
        close();
        emit({ type: "workspace_changed" });
      }
      if (k?.kind === "browser") await removeAllFiles(req.id);
      await forgetKnown(req.id);
      if ((await getMeta<string>("last")) === req.id) await setMeta("last", null);
      return null;
    }
    case "call": {
      const c = current;
      if (!c) throw new Failure("nothing_open", "no workspace is open");
      if (CHANGES.has(req.method)) {
        if (c.readOnly) throw new Failure("refused", "This workspace is open in another tab, so it can only be looked at here. Close it there to change it here.");
        // On top of whatever someone else has done to the files.
        await sync();
      }
      const r = JSON.parse(engine.call(req.method, JSON.stringify(req.args ?? {}))) as { value: unknown; effect: { graph: boolean; proposals: boolean }; writes: Writes };
      await persist(r.writes);
      if (r.effect.graph) emit({ type: "graph_changed", by: "you" });
      if (r.effect.proposals) emit({ type: "proposals_changed" });
      return r.value;
    }
    case "files":
      if (!current) throw new Failure("nothing_open", "no workspace is open");
      return current.store.list();
    case "file":
      if (!current) throw new Failure("nothing_open", "no workspace is open");
      return current.store.read(req.path);
    case "export":
      if (!current) throw new Failure("nothing_open", "no workspace is open");
      return { name: current.known.name, files: JSON.parse(engine.files()) };
    case "visible":
      visible = req.visible;
      return null;
    case "sync":
      await sync();
      return null;
  }
}

// One thing at a time, so a look at the files never lands in the middle of a change.
let chain: Promise<unknown> = Promise.resolve();
function serial<T>(f: () => Promise<T>): Promise<T> {
  const p = chain.then(f);
  chain = p.catch(() => {});
  return p;
}

/** The engine itself stopped part way, such as by running out of stack, so it can't be used again. */
function trapped(e: unknown): boolean {
  return e instanceof WebAssembly.RuntimeError || e instanceof RangeError || (e instanceof Error && /recursive use of an object|unreachable/.test(e.message));
}

/** Start the engine again, with the open workspace as its files say, which is as far as was written. */
async function restart() {
  try {
    engine.free();
  } catch {
    /* it's past freeing */
  }
  engine = new Engine();
  const c = current;
  current = null;
  if (c) {
    c.release();
    c.stopWaiting?.();
    c.observer?.disconnect();
    await open(c.known, c.store);
  }
}

self.onmessage = (m: MessageEvent<ToWorker>) => {
  const { seq, request } = m.data;
  serial(() => handle(request)).then(
    (value) => post({ seq, ok: true, value }),
    async (e) => {
      if (trapped(e)) {
        console.error("the engine stopped, so it's starting again:", e);
        await serial(restart).catch((x) => console.error("couldn't start the engine again:", x));
        post({ seq, ok: false, error: { kind: "other", message: `That was too much to work out here, so the engine started again with the workspace as it was last saved. (${errorOf(e).message})` } });
      } else post({ seq, ok: false, error: errorOf(e) });
    },
  );
};

// Another tab changed a workspace kept in this browser, or changed a folder's files.
channel.onmessage = (m: MessageEvent<{ changed: string }>) => {
  if (current && m.data.changed === current.known.id) serial(() => sync());
};

// Without a way to be told, a folder is looked at every second while the tab is showing.
setInterval(() => {
  const c = current;
  if (!c || !visible || !(c.store instanceof FolderStore)) return;
  if (c.observer && Date.now() % 5000 >= 1000) return;
  serial(() => sync()).catch((e) => console.warn(errorOf(e).message));
}, 1000);
