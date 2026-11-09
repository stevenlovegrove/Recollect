# Recollect

Recollect organises ideas from coarse to fine. You add a high-level idea, and Claude (over MCP) helps you decompose it into parts and alternatives. Recollect then shows which work is safe to start while decisions are still open, when a decision starts to block progress, and what's missing. Decisions stay open exactly: every value is the set of worlds where it's true, and you can pin any choice to see everything as it would be then, with nothing saved. See [DESIGN.md](DESIGN.md).

## Run

```sh
cargo build
(cd web && npm install && npm run build)
target/debug/recollect serve           # UI http://127.0.0.1:7878/   MCP http://127.0.0.1:7878/mcp
```

It opens on a choice of sample workspaces: plans, spreadsheets and puzzles. Opening one works on a copy in `~/.recollect`, so the originals in `examples/` don't change. A copy from before samples last changed is moved to `~/.recollect/samples-superseded`. To make your own:

```sh
target/debug/recollect init ~/ideas --systems plan,schedule
target/debug/recollect serve --workspace ~/ideas
```

Connect Claude Code:

```sh
claude mcp add --transport http recollect http://127.0.0.1:7878/mcp
```

Then ask Claude to call `get_overview` and build or critique the graph. Its changes arrive in the **Inbox** tab as proposals, which you accept in full, accept in part, or reject.

UI development: run `npm run dev` in `web/`. Vite serves on :5173 and proxies `/api` and `/mcp` to the backend. Set `RECOLLECT_BACKEND` if it isn't on :7878.

## Layout

| Path | What |
|---|---|
| `crates/recollect-graph` | The engine: nodes and edges with components and their cells, memoised computation, worlds as a value type, and the action log. See [docs/components.md](docs/components.md). |
| `crates/recollect-systems` | What a workspace can be made of: notes, formulas, planning and scheduling, plus the samples. See [docs/expressions.md](docs/expressions.md) for formulas. |
| `crates/recollect-server` | The `recollect` binary: workspaces, REST and WebSocket events, MCP, and watching files. |
| `web/` | React, React Flow and ELK. |
| `examples/` | The sample workspaces. To write them again, delete them and run `cargo run -p recollect-systems --example write_samples -- examples`. |

A workspace is plain files, so put it in git. Hand edits, `git pull` and other tools are picked up live, and recorded in the log.
