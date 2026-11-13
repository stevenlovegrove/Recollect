# Recollect

Recollect organises ideas from coarse to fine. You add a high-level idea, and Claude (over MCP) helps you decompose it into parts and alternatives. Recollect then shows which work is safe to start while decisions are still open, when a decision starts to block progress, and what's missing. Decisions stay open exactly: every value is the set of worlds where it's true, and you can pin any choice to see everything as it would be then, with nothing saved. See [DESIGN.md](DESIGN.md).

There are two ways to run it, with the same UI and the same engine:

- **As a static site.** The engine runs in the browser, compiled to WebAssembly. Workspaces are kept in the browser, or in folders on your computer in browsers that can open them (Chrome and Edge). It needs no server, so it can be hosted anywhere that serves files.
- **With `recollect serve`.** The engine runs in a local server, which also serves Claude over MCP.

## As a static site

Building needs Rust with the `wasm32-unknown-unknown` target, [wasm-pack](https://github.com/rustwasm/wasm-pack), and Node.

```sh
rustup target add wasm32-unknown-unknown
cd web
npm install
npm run build                  # the site is web/dist
npx vite preview               # or serve web/dist with anything
```

`npm run dev` does the same with Vite's dev server. It opens on a choice of sample workspaces, each a working copy kept in the browser, so it can be changed and started again. Your own can be kept in the browser too, or in a folder:

- **In a folder**, a workspace is plain files you can put in git, and changes others make to them show up as they happen, so you can watch Claude or an editor work on it. Each is recorded as an edit by `disk`, and can be undone.
- **In the browser**, it stays in that browser's storage. To move one in or out, save it as a zip or import one, from the workspace menu.

A workspace open in two tabs can only be changed in the first; the other looks, and takes over when the first closes.

A note with no links is there to be read rather than to hold anything together, so the map shows it in full, as markdown, to the left of everything else. That's where each sample says what it is and what to try: its entry in the list of samples is one line.

The GitHub Actions workflow in `.github/workflows/pages.yml` builds the site and publishes it to GitHub Pages.

## With `recollect serve`

```sh
cargo build
(cd web && npm install && npm run build)
target/debug/recollect serve           # UI http://127.0.0.1:7878/   MCP http://127.0.0.1:7878/mcp
```

Samples opened here are working copies in `~/.recollect`, so the originals in `examples/` don't change. A copy from before samples last changed is moved to `~/.recollect/samples-superseded`. To make your own:

```sh
target/debug/recollect init ~/ideas --systems plan,schedule
target/debug/recollect serve --workspace ~/ideas
```

Connect Claude Code:

```sh
claude mcp add --transport http recollect http://127.0.0.1:7878/mcp
```

Then ask Claude to call `get_overview` and build or critique the graph. Its changes arrive in the **Inbox** tab as proposals, which you accept in full, accept in part, or reject. The same folder can be open in the static site at the same time: each sees the other's changes.

UI development against the server: `RECOLLECT_BACKEND=http://127.0.0.1:7878 npm run dev` in `web/`. Vite serves on :5173 and proxies `/api` and `/mcp` to it.

## Layout

| Path | What |
|---|---|
| `crates/recollect-graph` | The engine: nodes and edges with components and their cells, memoised computation, worlds as a value type, and the log of changes, for undo and redo. See [docs/components.md](docs/components.md). |
| `crates/recollect-systems` | What a workspace can be made of: notes, formulas, planning and scheduling, plus the samples. See [docs/expressions.md](docs/expressions.md) for formulas. |
| `crates/recollect-core` | A workspace: its files, proposals, and every call the app and Claude make of it, through one `dispatch`. No I/O: it's given files and says which to write. |
| `crates/recollect-wasm` | `recollect-core` for the browser. |
| `crates/recollect-server` | The `recollect` binary: workspaces on disk, REST and WebSocket events, MCP, and watching files. |
| `web/` | React, React Flow and ELK. `web/src/engine` runs the engine in a worker, and keeps workspaces in the browser or in folders. |
| `examples/` | The sample workspaces. To write them again, delete them and run `cargo run -p recollect-systems --example write_samples -- examples`. |
