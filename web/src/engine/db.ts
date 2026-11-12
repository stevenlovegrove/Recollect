// What this browser keeps: the files of workspaces kept in it, which workspaces it knows about
// (including folders, by their handles), and a few settings. Used by the page and by the worker.

export type Files = Record<string, string>;

/** A workspace this browser knows: one kept in it, or a folder on this computer. */
export interface Known {
  id: string;
  name: string;
  kind: "browser" | "folder";
  /** For a person to read: where it is. */
  location: string;
  /** Set for a sample's working copy. */
  sample?: string;
  /** For a folder: the handle to open it with again. */
  handle?: FileSystemDirectoryHandle;
  /** Milliseconds since 1970. */
  opened: number;
}

const NAME = "recollect";
const FILES = "files";
const KNOWN = "workspaces";
const META = "meta";

let opening: Promise<IDBDatabase> | null = null;

function db(): Promise<IDBDatabase> {
  opening ??= new Promise((resolve, reject) => {
    const req = indexedDB.open(NAME, 1);
    req.onupgradeneeded = () => {
      const d = req.result;
      d.createObjectStore(FILES);
      d.createObjectStore(KNOWN, { keyPath: "id" });
      d.createObjectStore(META);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => {
      opening = null;
      reject(req.error ?? new Error("this browser won't keep anything (IndexedDB is unavailable)"));
    };
  });
  return opening;
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onabort = tx.onerror = () => reject(tx.error ?? new Error("couldn't save in this browser"));
  });
}

function result<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

const range = (ws: string) => IDBKeyRange.bound([ws, ""], [ws, "￿"]);

/** Every file of a workspace kept in this browser. */
export async function readFiles(ws: string): Promise<Files> {
  const d = await db();
  const tx = d.transaction(FILES, "readonly");
  const store = tx.objectStore(FILES);
  const [keys, values] = await Promise.all([result(store.getAllKeys(range(ws))), result(store.getAll(range(ws)))]);
  const out: Files = {};
  keys.forEach((k, i) => (out[(k as [string, string])[1]] = values[i] as string));
  return out;
}

/** One file, or nothing. */
export async function readFile(ws: string, path: string): Promise<string | null> {
  const d = await db();
  return ((await result(d.transaction(FILES, "readonly").objectStore(FILES).get([ws, path]))) as string | undefined) ?? null;
}

/** Write and remove files together: either all of it lands, or none does. */
export async function writeFiles(ws: string, write: Files, remove: string[] = []): Promise<void> {
  const d = await db();
  const tx = d.transaction(FILES, "readwrite");
  const store = tx.objectStore(FILES);
  for (const [path, text] of Object.entries(write)) store.put(text, [ws, path]);
  for (const path of remove) store.delete([ws, path]);
  await done(tx);
}

export async function removeAllFiles(ws: string): Promise<void> {
  const d = await db();
  const tx = d.transaction(FILES, "readwrite");
  tx.objectStore(FILES).delete(range(ws));
  await done(tx);
}

export async function known(): Promise<Known[]> {
  const d = await db();
  const all = (await result(d.transaction(KNOWN, "readonly").objectStore(KNOWN).getAll())) as Known[];
  return all.sort((a, b) => b.opened - a.opened);
}

export async function getKnown(id: string): Promise<Known | undefined> {
  const d = await db();
  return (await result(d.transaction(KNOWN, "readonly").objectStore(KNOWN).get(id))) as Known | undefined;
}

export async function putKnown(k: Known): Promise<void> {
  const d = await db();
  const tx = d.transaction(KNOWN, "readwrite");
  tx.objectStore(KNOWN).put(k);
  await done(tx);
}

export async function forgetKnown(id: string): Promise<void> {
  const d = await db();
  const tx = d.transaction(KNOWN, "readwrite");
  tx.objectStore(KNOWN).delete(id);
  await done(tx);
}

export async function getMeta<T>(key: string): Promise<T | undefined> {
  const d = await db();
  return (await result(d.transaction(META, "readonly").objectStore(META).get(key))) as T | undefined;
}

export async function setMeta(key: string, value: unknown): Promise<void> {
  const d = await db();
  const tx = d.transaction(META, "readwrite");
  tx.objectStore(META).put(value, key);
  await done(tx);
}
