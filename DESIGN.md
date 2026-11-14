# Recollect: design

Recollect organises ideas into a structured graph, and helps develop them with an LLM over MCP. You add coarse ideas, and they get drilled down until you can see which work is safe to start while decisions are still open, when a decision starts holding things up, and what's missing.

The engine underneath is described in [docs/components.md](docs/components.md). This document is about the app built on it.

## The idea everything rests on

**A decision stays open, exactly.** Each open choice is a variable, and a *world* is one way all of them could land. Any value can differ between worlds, and is stored as the exact set of worlds where each answer holds, so `need(static) = calib∈{cad,free}` is both the value and its explanation. Nothing is approximated, and nothing is written onto a decision's children, so reopening it brings everything back with nothing to undo.

**Looking at a world saves nothing.** Pin any choices and every value in the app reads as it would then. The engine works this out with the same code, by narrowing what it has, so a what-if and the real thing can't disagree. That one mechanism is the app's what-if for a plan, its filter for a budget, and how you explore a puzzle.

## Workspaces and systems

A workspace is a directory of plain files:

```
recollect.toml       its name, and which systems it has taken on
nodes/<id>.yaml      a node each, with its edges, as plain data that can be edited by hand
```

The files are what's true. While a workspace is open, every change is an event in a log kept in memory, saying who made it and why, which is what undo and redo work from: undoing is a change too, so it can be redone. Editing a file by hand, another program writing one, or a `git pull`, is noticed and recorded as an event under the author `disk`, which can be undone like anything else. A workspace lives happily in git, and git is what keeps its history between sessions.

The same engine runs in two places. In a browser, as a static site, it's WebAssembly in a worker, and a workspace is kept in the browser or in a folder on the computer. With `recollect serve`, it's in the server, which also serves Claude over MCP. Either way the engine is `recollect-core`, which does no I/O: it's given a workspace's files and says which to write after each change, so the browser and the server only read and write files.

Nodes and the edges between them are both entities, and each has components, such as `task` on a node or `part_of` on an edge. A component brings cells, so `task` brings `estimate` and an `alternative` edge brings `weight`, and a cell is only there on an entity with its component. A **system** provides components, their cells and their checks. Nobody chooses them: a workspace takes one on the first time it uses something from it, and says so in `recollect.toml`. Every component is on offer from the start, and a change that uses a new one is refused whole, system and all, if it would be refused anyway:

| System | What it adds |
|---|---|
| `notes` | Always there. `named` on every node, with its `name`, `notes` with a `body`, `sources`, `drawing`, and edges that only say how ideas relate: `serves`, `supports`, `contradicts`, `constrains`, `supersedes`, `relates` |
| `formulas` | Always there. `value`, typed in or worked out by a formula, which can be a distribution ([docs/distributions.md](docs/distributions.md)), `label` edges to name what a formula reads, `options` for a choice, `requires` for what your choices have to meet and `evidence` for what's known, which between them say which worlds count, `macro` for a function of your own, and `is_a` for a kind. See [docs/expressions.md](docs/expressions.md) |
| `plan` | Taken on with its first component. `part`, `work`, `task`, `goal`, `question`, `risk`, `unknown`, `decision`, `uncertain` and `option`, with `part_of`, `alternative` and `depends_on` edges. Works out what's needed, completion, blocking, what can start, scope and pressure |
| `schedule` | Taken on with the first task, since a task's estimate is its duration. Dates and slack in every world, from `plan`'s facts. It has no component of its own, so it isn't on before then: a puzzle's choices would otherwise have dates worked out for nothing |

A component also says what it means to someone looking at it: what it's for, its icon, which edges give the graph its shape, which say what comes first, and which cells are worth a line on a card. A map is laid out by both kinds of edge, and a tree like the Outline only by shape, because waiting for something isn't being part of it. The app reads that rather than knowing any of it, so a plugin's components would get the same treatment. A component no system knows about is kept and shown as it is.

## The plan model

- **`part_of`:** child to parent. All of a node's parts are needed.
- **`decision` and `alternative`:** a decision's options are ordinary nodes, each with an `alternative` edge to it, which holds the option's `weight` and whether it's `ruled_out`. Only the option taken is needed.
- **Shared work is just two parents.** Work under two options is `part_of` each, and is safe to do before deciding. Finding that overlap is a big part of the value.
- **`unknown`:** a gap. It says something here isn't specified yet, it can't be done or chosen, and removing it is the claim that it's closed, which has an author and a reason like anything else.
- **`done` is a cell, worked out unless a node says otherwise.** It comes from what's beneath, and reads as finished, still owed, or undefined for something with nothing to finish, which is what a plain note is. Typing over it replaces that, with `true`, `false` or a formula, and removing what was typed brings it back. If what it says disagrees with what's beneath, that's a notice. `dropped` works the same way.
- **Blocking is only inherited through parents in play,** so shared work doesn't wait on a phase this world isn't doing.

## Formulas

A spreadsheet and a puzzle aren't separate systems here. Each is nodes with values, some of them formulas, and a puzzle's solutions are the worlds where a `requires` node is true. Because a formula can read a plan's cells and fill its `estimate`, a plan, its costs and its constraints are one graph: `sum(t in task: if(t.need, t.estimate, 0))`, on a node that labels each task, is its effort in every world, with shared work counted once, and a `requires` node saying that's at most 6 rules out the options that can't meet it. Claude writes formulas the same way a person does, as text, with a reason in the log. A person types them as in a spreadsheet: in the Inspector or the Table, `=` starts a formula and anything else is a value, and a leading `'` means the rest is literal text. See [docs/expressions.md](docs/expressions.md).

A cell holds a typed value or a formula, and that's the same whether a person wrote it or a system did. A component's cell can default to a formula, which is how dates and `done` work, so what's worked out can be read, overridden and restored like anything else. Some defaults are built into a system instead: what makes worlds, such as `choice`, and for now `need`, the blocking cells and `scope`.

## Views

| Tab | Shows | When |
|---|---|---|
| **Map** | The graph, laid out by the edges that give it shape or say what comes first, reading down or across. The Links menu chooses which edges shape it, each remembered per workspace in that browser. What a world rules out dims, and nothing moves | Always |
| **Outline** | The same structure, coarse to fine | Always |
| **Table** | Nodes against cells. Values and formulas edit in place, as in a spreadsheet, and what just ran is highlighted. The rows can be narrowed to what differs between the worlds left, or in any world, which is what pinning a choice changes; or a table can be shown as a grid of its rows | Always |
| **Decisions** | Each decision, what each option needs, and whether it's holding work up. Deciding shows what it would change first | Once there's a decision |
| **Work** | Tasks by where they stand: can start, depends on a decision, waiting, finished, not needed | Once there's work |
| **Timeline** | Dates as a chart, with hatching for how far a date could move | Once there's a task |

Three more look after the workspace rather than work in it, so they're behind ⋯ at the end of the tabs (in More, on a phone):

| View | Shows |
|---|---|
| **Files** | The workspace's files as they are, read-only, for debugging |
| **History** | Every change since the workspace was opened, with undo and redo (⌘Z, ⇧⌘Z), and any one change can be taken back on its own |
| **Checks** | Invariants that are broken, and notices worth a look. Anything broken also shows as a count on ⋯, so it isn't hidden by the menu |

**The worlds chip** heads every page, and says how many worlds are left and what's pinned. On the map it floats at the top left, across from the map's own view options (search, which nodes and links, focus) at the top right; each has a dot when it's narrowing what's shown. It opens (or `w`) into one panel, the same on every page: every open choice and what's still possible, where clicking an option pins it. Choices with the same options become a grid, and choices with an `at` are placed on a board. A workspace's own [drawings](docs/drawings.md) can be picked instead, and the choice is remembered. A puzzle's solutions can be stepped through, and for what's uncertain it says what's most worth finding out next. Pinning saves nothing.

## Claude, over MCP

`recollect serve` serves the web UI and an MCP server at `/mcp`, so the UI updates live as Claude works. A browser with the same folder open sees Claude's changes too, as changes to the files. Claude reads with `get_overview`, `get_node`, `get_tree`, `get_work`, `get_decisions`, `read_world`, `find_checks`, `get_history` and search, most of which take a world, so it can answer a what-if the same way the app does.

It writes with `apply`, which commits a change-set at once with its reason, so the user sees it land and can take any one change back from History. A change-set that wouldn't apply, or that has a formula with a type error or an unknown input, is refused with the reason. (Changes used to wait for review in an inbox; `llm_direct_writes` in an older `recollect.toml` is now ignored.) Ops are the log's seven (`spawn`, `despawn`, `add`, `remove`, `link`, `unlink` and `set`), plus a little to make them easier to write: names for handles, `new:x` for a node that doesn't exist yet, `add_node`, `delete_node`, `table`, and `formula` with `labels` to set a formula and say what its names mean as one change. Writing in a cell of a component a node doesn't have gives it that component. See `crates/recollect-core/src/ops.rs`.

Whichever way a change arrives, the rules are the same: it's all or nothing, and only what it newly breaks is refused, so a hand edit that broke something doesn't block other work.

## Layout

```
crates/
  recollect-graph         The engine: entities, components and cells, memoisation, worlds, the log. Knows nothing of planning
  recollect-graph-macros  #[cell] and #[component]
  recollect-systems       notes, formulas, plan, schedule, views, and the samples
  recollect-core          A workspace: its files, views, and every call through one `dispatch`. No I/O
  recollect-wasm          recollect-core for the browser
  recollect-server        The `recollect` binary: workspaces on disk, REST, WebSocket events, MCP
examples/                 The sample workspaces, written by recollect-systems' write_samples
web/                      Vite, React, React Flow and ELK. `src/engine` runs the engine in a worker, with its stores
```

## The samples

The example problems are real workspaces in `examples/`, offered when nothing's open and under the workspace menu. Opening one makes a working copy, kept in the browser or, with `recollect serve`, in `~/.recollect/samples`, so the originals never change, and it can be started again. `tests/samples.rs` checks what's committed still means what it should: an invoice that totals 96, a budget over £200 exactly when `Stay=Hotel`, 92 queens, 288 grids, the Japanese owning the zebra, two dice totalling seven one time in six, and savings whose log is exactly a bell curve.

## What was dropped from the first version

The first version had its own model, and these didn't survive the move to the engine:

- **Option scopes on `part_of`, and options as a map on the choice.** Options are nodes, which is what lets them have their own evidence and risks.
- **Robustness as a number.** It assumed paths were independent, which gave wrong answers. Probability is now worked out exactly from the formula, and only where someone gives weights.
- **Status (`live`, `parked`, `dropped`, `superseded`) and per-component states.** `dropped` and `done` are cells, and a `supersedes` edge says the rest.
- **Tags, aliases, wikilinks and `created_by`.** A name is a cell, and the log says who did what.
- **`merge_nodes`.** It can come back as a change-set that moves edges, with no special op.
- **`outcome`, `actionable`, `uncertain`, `principle` and `artifact`, with presets.** Now `goal`, `task` and `question`, with `constrains` for a principle. Confidence on a question went too.
- **Markdown files with frontmatter.** Plain YAML, one file per node.
- **Proposals and the Inbox.** Claude's changes waited in `proposals/` to be accepted whole or in part. Nobody reviewed them op by op, so they now land directly, and History takes any one back. A leftover `proposals/` folder is ignored.
- **A log on disk, and history as a tree.** The log was the source of truth, kept in `log.jsonl`, and you could go back to any event and branch from it. Now the files are what's true, the log is for the session, for undo and redo, and history between sessions is git's.
- **Client-side what-ifs.** The engine narrows by world instead, which is exact.
- **Some gap checks.** Those that needed timestamps or authors on nodes (stale work, ungrounded LLM nodes), and leverage across outcomes, could come back as checks. The rest became notices in `plan`.

## Not built yet

- **Ingesting past sessions, memory and docs,** which is what the first version was heading towards.
- **History between sessions.** The log is only for while a workspace is open. Between sessions, history is git's, if it's in git.
- **Sharing a workspace between machines.** Files merged by git are fine, and are recorded as an edit. A workspace kept in a browser can be moved as a zip.
- **Claude, with only the static site.** MCP needs `recollect serve`. A static site with a folder open sees what Claude does through it.
- **A plugin mechanism.** Systems are compiled in. Cells read and written by name are what one would use, and formulas and macros cover much of what people would write one for.
