# Plan: abstracted storage, and Recollect as a static site

Status: built. What follows is the plan as it was agreed. Where the work went differently:

- **Long chains on the web.** Raising the wasm stack size with a linker flag did nothing, because wasm calls run on the JavaScript engine's own stack, which a page can't raise, and 3,000 levels of parts ran out of it. Instead the engine works long chains out in stretches: a cell too deep to finish is put off, worked out from the top, and the read tried again (`Graph::reading` in `recollect-graph/src/graph.rs`). Natively there's no limit, so nothing changes there. `tests/depth.rs` checks the answers are the same at a limit of 5 for every sample. 10,000 levels takes about 13 s in wasm against 6 s natively. If the engine traps anyway, the worker starts it again from the saved files.
- **A node keeps the file it came from.** A file made by hand or by an agent, like `nodes/an-idea.yaml`, is read as its node and written back to, rather than copied to `nodes/<id>.yaml`. Otherwise the agent's later edits to its own file would lose to the copy.
- **Two tabs.** The second tab takes over by itself when the first closes, rather than asking to be reloaded.
- **Undo and redo** are in the engine (`Log::undo_last`, `Log::redo`), each an ordinary checked event, and on ⌘Z and ⇧⌘Z.
- **Debugging.** The page's backend is `window.recollect`, and `openFolder` and `createInFolder` take a directory handle, so a folder workspace can be driven without the picker, such as with the private file system the browser gives each site (OPFS).

## Where things stand

All of the work happens in the Rust server. The browser is a thin client: every view, what-if, sweep and edit is a `fetch` to `/api/*` (`web/src/api.ts`), and changes arrive over a WebSocket. Storage is tied to the local filesystem in three places:

| Where | What it does with the disk |
|---|---|
| `recollect-graph/src/log.rs` (`Disk`, `Log::open`, `reload`, `checkout`) | `log.jsonl` as the source of truth, `log.applied`, `log.lock`, crash repair, fsync, branching history, and diffing hand edits into "external edit" events |
| `recollect-graph/src/saved.rs` | `nodes/<id>.yaml`, written atomically, only when the text changed |
| `recollect-server/src/app.rs`, `files.rs` | `recollect.toml`, `proposals/*.json`, sample working copies in `~/.recollect`, the `notify` file watcher, the remembered last workspace, and the Files tab |

So a static site needs more than a new storage layer. **The engine has to run in the browser**, which means compiling it to WebAssembly. Storage then becomes a small async interface on the TypeScript side.

### What already works in our favour

- **The engine compiles to wasm.** `cargo check -p recollect-systems --target wasm32-unknown-unknown` passes once `uuid` gets its `js` feature. I tried that with a temporary edit and reverted it. `Mutex` is harmless single-threaded, and `stacker` does nothing on wasm (see risks).
- **The YAML files are the full state.** `nodes/*.yaml` is a complete projection, so dropping the log loses history and nothing else, and the file format doesn't change.
- **`Log::new` already runs in memory only**, with commit, invariant checks, what-if and undo. `Log::reconcile` already turns "files differ from the graph" into ops.
- **Samples are code.** `samples::ops_of` builds each sample from scratch, so the browser can create one with no files to fetch.
- **`api.ts` is already a narrow async RPC surface.** If it keeps its shape and only its transport changes, the views barely change.

## The shape of the change

```
           main thread                               Web Worker
┌───────────────────────────┐  postMessage  ┌──────────────────────────────────┐
│ React UI                  │ ────────────▶ │ recollect-wasm (the engine)      │
│ api.ts → Backend          │ ◀──────────── │   Workspace: config, Log,        │
│ (picks folders: needs a   │   events      │   proposals, views               │
│  user gesture)            │               │        │ writes (path → text)     │
└───────────────────────────┘               │        ▼                         │
                                            │ FileStore (async, TS)            │
                                            │   BrowserStore │ FolderStore     │
                                            │   (IndexedDB)  │ (File System    │
                                            │                │  Access API)    │
                                            └──────────────────────────────────┘
```

**The key decision: Rust stays synchronous and knows nothing about storage. The store is a dumb async file store in TypeScript.** Browser storage APIs are async-only (IndexedDB, and the File System Access API outside of OPFS sync handles), and sync Rust in wasm can't wait on them. So the Rust side:

- is loaded from a set of files (`path → text`),
- after each change, returns the files to write and delete (`path → text`, already serialised as YAML or JSON in Rust so the format can't drift),
- and the worker hands those to the store.

Because the store only sees paths and text, the Files tab comes for free. The same layout works for an in-browser workspace, a real folder, and the native server.

### The storage interface

```ts
/** One workspace's files, as paths relative to its root, with `/` between folders. */
interface FileStore {
  readonly kind: "browser" | "folder";
  list(): Promise<{ path: string; size: number }[]>;
  read(path: string): Promise<string | null>;
  /** Apply one change's writes together. BrowserStore does this in one IndexedDB transaction. */
  commit(writes: Record<string, string>, deletes: string[]): Promise<void>;
}

/** Finding, making and reopening workspaces. */
interface Workspaces {
  recent(): Promise<WorkspaceRef[]>;          // browser ones, and folders whose handles were kept
  create(where: "browser" | FileSystemDirectoryHandle, name: string, systems: string[]): Promise<WorkspaceRef>;
  open(ref: WorkspaceRef): Promise<FileStore>; // may ask for folder permission again
  remove(ref: WorkspaceRef): Promise<void>;    // browser ones only
}
```

- **BrowserStore** uses IndexedDB, with one object store keyed by `[workspaceId, path]` and one transaction per commit, so each change lands whole. It works in every current browser.
- **FolderStore** uses `showDirectoryPicker()` with `{ mode: "readwrite" }`. The handle is kept in IndexedDB so the folder can be reopened, and each file is written through `createWritable()`, which only replaces it on `close()`. It's Chromium-only (Chrome and Edge), so Firefox and Safari get BrowserStore plus import and export (see phase 5).
- The picker needs a user gesture, so it runs on the main thread. The handle can be cloned, so it's posted to the worker, which owns all reading and writing.
- A shared contract test suite runs against both. FolderStore can be tested against an OPFS directory (`navigator.storage.getDirectory()`), which gives the same `FileSystemDirectoryHandle` API with no picker.

### Why a worker

Views on large puzzles (zebra, sudoku) take noticeable time. On the main thread they would freeze the UI. In a worker, the existing "only the newest answer counts" logic in `workspace.ts` keeps working as it is. The worker also gives a single owner for the open workspace, which replaces the `Mutex<Option<Open>>` in `App`.

## Phases

Each phase leaves the app working and the tests passing.

### 1. Make the engine platform-neutral (`recollect-graph`)

- `uuid`: add the `js` feature for `wasm32`, through a target-specific dependency.
- Replace `SystemTime::now()` (`log.rs:523`, `log.rs:591`), which panics on `wasm32-unknown-unknown`, with `web-time`, or take the time from the caller.
- **Remove the on-disk log**: `Disk`, `Log::open*`, `read_log`, `take_lock`, `crash_after`, `reload`, `log.applied`, `log.jsonl`, and from `saved.rs` `save_dir`, `load_dir`, `write_files`, `write_atomically`, `sync_dir` and `remove_temp_files`. The YAML (de)serialisation of `SavedNode` stays.
- **History becomes linear and lasts only for the session**, kept for undo and redo. Remove `parent`, `Record::Head`, `checkout`, `newest_after`, `state_at` and branch handling. Undo commits the inverse of the newest event not yet undone, and redo commits the inverse of that undo, so both are ordinary events, can be checked, and write files like any other change. A new change clears the redo stack.
- Add `Log::from_saved(SavedGraph, registry) -> (Log, Vec<LoadProblem>)`, which starts with the loaded graph and an empty history.
- Make the touched entities from `record` available (for example `Log::take_dirty() -> BTreeSet<Entity>`) so the caller knows which node files to rewrite. `write_only` already works this out, so the logic moves rather than being written fresh.
- Tests: `crash.rs` (10) and `concurrency.rs` (5) go, since they test a process crashing or racing on the log file. The disk parts of `saving.rs` and `log.rs` move to the new load-and-write path. The in-memory tests stay unchanged.

### 2. Pull the app out of the server (new crate `recollect-core`)

Move what isn't HTTP into a crate with no `tokio`, `axum`, `notify` or `std::fs`:

- `ops.rs` as it is, and `views.rs` as it is, except that `Open` becomes `Workspace` and `dir` becomes an `id` plus a display `location`.
- From `app.rs`: `Config`, `Proposal`, `formulas_work`, `refusal`, and `apply`, `propose`, `accept`, `reject` and `undo` as methods on `Workspace`.
- `Workspace::load(files: BTreeMap<String, String>) -> Result<(Workspace, Vec<LoadProblem>)>`. It parses `recollect.toml` (the `toml` crate is pure Rust), `nodes/*.yaml` and `proposals/*.json`, and ignores `log.*`.
- Every mutating method returns `Writes { write: BTreeMap<path, text>, delete: Vec<path> }`.
- `Workspace::new_files(name, systems)` and `Workspace::sample_files(name)` produce the files of a new workspace (the latter from `ops_of`).
- `Workspace::reconcile(files)` handles a reread folder, using the existing `Log::reconcile`. Anything changed by hand becomes an event under `disk`, as now.
- The logic in the server's `tests.rs` (45 tests) moves here and runs against an in-memory file map. Tests that are only about HTTP stay with the server.

### 3. Engine in the browser, stored in IndexedDB

- New crate `recollect-wasm`, a `wasm-bindgen` façade over `recollect-core` with JSON in and out, one function per current endpoint: `open(files)`, `view(world)`, `plan(world)`, `apply(ops, why)`, `try`, `sweep`, `rows`, `history`, `proposals`, `accept`, `reject`, `undo`. It's built with `wasm-pack --target web` into `web/src/engine/pkg`, and run from `npm run build`.
- `web/src/engine/worker.ts`: loads the wasm, holds the open workspace and its `FileStore`, and handles messages. Each mutation applies in memory and then awaits `store.commit(writes)`. Commits are queued so they land in order.
- `api.ts`: put a `Backend` interface under the existing `api` object, with a `WorkerBackend` that does request/response over `postMessage`, and an `HttpBackend` that calls `POST /api/call/{method}` on the server (see "Agents"). The method names and types stay the same, so the views don't change. `subscribe()` becomes events posted by the worker: `graph_changed`, `proposals_changed` and `workspace_changed` keep their meanings.
- Samples: each is a BrowserStore workspace with id `sample:<name>`, made from `Workspace::sample_files`. "Start again" deletes it and remakes it. `FORMAT` still decides when an old copy is out of date.
- Remember the last workspace in `localStorage` in place of `~/.recollect/last`.
- `vite.config.ts`: `base: "./"` so `dist/` works under any path, as on GitHub Pages. Routing is already by `location.hash`.
- **Milestone: `web/dist` served by any static file server opens a sample, edits it, survives a reload, and makes and reopens a browser workspace.**

### 4. Local folders

- "Open folder…" and "New in folder…" in `Welcome.tsx` and the workspace menu, replacing the typed path.
- Recent folders, from kept handles. On reopen, `queryPermission`, and then `requestPermission` from a click if needed.
- **Live file changes**, so that an agent or editor working on the folder shows up as it happens. Behind a `watch(onChange)` method on `FolderStore`:
  - Where `FileSystemObserver` exists (feature-detected), observe `nodes/` and `proposals/` recursively. It ran as a Chrome origin trial (129–134) and isn't in a spec yet, so it can't be the only way.
  - Otherwise, poll while the tab is visible: about every second, list `nodes/` and `proposals/` and compare each file's `lastModified` and `size` with what was last seen. That only reads metadata, and it reads a file's text only when the file has changed. On focus, poll at once.
  - Changed files go to `Workspace::reconcile`. It diffs against the graph, so the app's own writes are never mistaken for edits, and other people's changes become an event under `disk`, with the same debounce as the server's 150 ms. That replaces the `notify` watcher.
  - BrowserStore workspaces are out of reach of anything outside the page, so agent work needs a folder workspace.
- One writer per workspace: take a Web Lock (`navigator.locks`) named for the workspace, in place of `log.lock`. A second tab says the workspace is open elsewhere and shows it read-only. `BroadcastChannel` tells the other tabs to refresh after a change.
- Write a `.gitignore` only when making a folder workspace, and not with `log.lock` and `log.applied` any more.

### 5. UI and docs

- `workspace.dir` is used as a key for remembered choices (`MapView.tsx:357`, `WorldsView.tsx:55`), so use the workspace id instead.
- The status dot (`App.tsx:227`) means "connected" now. It becomes saved, saving, or couldn't save, with the reason (such as folder permission revoked), and a `beforeunload` warning while writes are pending.
- History: the History tab lists this session's events, with Undo and Redo (and ⌘Z / ⇧⌘Z in `Shortcuts.tsx`). The "you've gone back" banner (`App.tsx:230`), checkout and the branch graph in `HistoryView.tsx` go. Per-node history in the Inspector stays, limited to the session.
- Files tab: unchanged apart from the backend, through `FileStore.list` and `read`.
- Import and export, for browsers without folder access and for moving a browser workspace into a folder: export as a `.zip` of the same layout, and import a `.zip` or a folder through `<input webkitdirectory>`.
- Rewrite DESIGN.md's "Workspaces and systems", "Claude, over MCP", "Layout" and "Not built yet", and the README's way to run it.

### 6. Hosting (optional)

- A GitHub Actions workflow that builds the wasm and the web app and publishes `web/dist` to Pages.

## Agents

Every MCP tool works through the engine, and none of them reads or writes files:

- **Reads are worked out:** `read_world`, `get_work`, `get_decisions` and `get_tree` narrow everything to a world, and `get_node` shows where each cell's value comes from.
- **Writes are checked:** `propose` and `apply` resolve names and `new:x`, expand `table` and `formula`-with-`labels`, refuse formulas with type errors, and refuse anything that newly breaks an invariant.

An agent that only edited the YAML would lose all of that. It couldn't answer a what-if, and its mistakes would land as hand edits, which can't be refused. So agents need the engine as well as the files.

**Decided: keep `recollect serve`**, with MCP over HTTP at `/mcp`, rebuilt on `recollect-core`. That gives two ways to run the same UI:

| | Static site | `recollect serve` |
|---|---|---|
| Engine | wasm in a Web Worker | native, in the server |
| UI backend | `WorkerBackend` | `HttpBackend` (REST and the WebSocket, as now) |
| Storage | BrowserStore or FolderStore | `FsStore`: native files, written atomically, with the `notify` watcher |
| Agents | none directly, but see below | MCP at `/mcp`, as now |

**One dispatch, so the two backends can't drift.** `recollect-core` gets `dispatch(&mut Workspace, method, args: Value) -> Result<(Value, Writes), Error>`, covering `view`, `plan`, `apply`, `try`, `sweep`, `rows`, `history`, `undo`, `redo`, `proposals`, `accept` and `reject`. The wasm façade is that one function. The server's REST becomes one route, `POST /api/call/{method}`, around it, plus its own routes for opening, making and listing workspaces, which differ by host. MCP tools call the same `Workspace` methods. `api.ts` sends the same method names whichever backend is in use. Contract tests run the same calls through both backends and compare the JSON.

**Choosing the backend at load:** the server adds `<meta name="recollect-backend" content="http">` to the `index.html` it serves. With no such tag, as on a static host, the app uses the worker. A query flag can override it for testing.

**Server and static tab on one folder.** This needs nothing extra. If a static tab has a folder open while `recollect serve` (and Claude through it) works on the same folder, each sees the other's writes as file changes and reconciles them as `disk` edits: the server through `notify`, and the tab through observing or polling. Each rereads before it writes. If both write one node file at the same moment, the last write wins, and the other side records the difference. That's the same "hand edit" path as today, now without a log.

**What changes in the server:**
- `app.rs`: `Open` becomes a `recollect-core` `Workspace` plus an `FsStore` and the watcher. `Log::open` and `log.lock` go. One process per folder is still enforced, with an advisory lock on a `.recollect.lock` file that isn't written into the workspace's data or `.gitignore`. Samples are made from `Workspace::sample_files` in place of copying `examples/`, so `--samples` goes.
- `api.rs`: the generic call route, workspace routes, `/api/events` and the Files routes. `/api/checkout` goes.
- `mcp.rs`: unchanged apart from `o.log` becoming `o.workspace`, and `get_history` now covering the session.
- `main.rs`: `serve` and `init` stay.
- `tests.rs`: the logic moves to `recollect-core` (phase 2), and the HTTP and MCP tests stay here.

## Decided

- **Server:** kept, as above.
- **History:** kept, for undo and redo within a session (phase 1 and phase 5).
- **Old `log.jsonl` files:** removed in one change, with no migration code. Delete `log.jsonl` and `log.applied` from `examples/*`, update `write_samples` to write through the new path, drop `log.lock` and `log.applied` from `.gitignore` handling, and drop the "made by an older version" check in `app.rs::load`.

## Risks

- **Stack depth.** `stacker` grows the stack for deep formula chains natively, but does nothing on wasm, so a long recursion (Fibonacci's F[80], deep `prev` chains) could overflow the default 1 MB wasm stack. Raise it with `-C link-arg=-zstack-size=…` and test every sample in the browser early, in phase 3.
- **Bundle size.** Release build with `wasm-opt -Oz` and `lto = true`, then measure. It's probably a few MB, which is fine for an app.
- **Partial folder writes.** Without the log, a tab closed in the middle of a multi-file write can leave some node files newer than others. Each change is usually one or two files, and the `beforeunload` guard covers most of it. Treat that as acceptable while versioning is out of scope. BrowserStore is atomic.
- **Folder access is Chromium-only.** Import and export are the fallback.
