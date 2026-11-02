# Recollect: design

Recollect organises ideas into a structured graph, and helps develop them with an LLM over MCP. You add coarse ideas, and they get drilled down until you can see which work is safe to start while decisions are still open, when a decision starts holding things up, and what's missing.

The engine underneath is described in [docs/graph-core.md](docs/graph-core.md). This document is about the app built on it.

## The idea everything rests on

**A decision stays open, exactly.** Each open choice is a variable, and a *world* is one way all of them could land. Any value can differ between worlds, and is stored as the exact set of worlds where each answer holds, so `need(static) = calib∈{cad,free}` is both the value and its explanation. Nothing is approximated, and nothing is written onto a decision's children, so reopening it brings everything back with nothing to undo.

**Looking at a world saves nothing.** Pin any choices and every value in the app reads as it would then. The server works this out with the same rules, by narrowing what it has, so a what-if and the real thing can't disagree. That one mechanism is the app's what-if for a plan, its filter for a budget, and how you explore a puzzle.

## Workspaces and systems

A workspace is a directory:

```
recollect.toml       its name, and which systems it uses
log.jsonl            what's true: every change, who made it, and why
nodes/<id>.yaml      a node each, as plain data that can be edited by hand
graph.yaml           what applies to every node
proposals/<id>.json  changes waiting for review
```

The log is the source of truth, and the YAML files are a projection of where it is now. History is a tree, like git's: going back to an earlier event moves everything there, files included, and changing something from there starts a new branch, with nothing lost. There's no separate read-only way of looking at the past, because you can just be there. Editing a file by hand, or a `git pull`, is noticed and recorded as an event under the author `disk`. A workspace lives happily in git.

A node has no kind. It's a bag of components, and a link is a bag of kinds. A **system** provides components and the rules over them, and a workspace lists the ones it uses:

| System | What it adds |
|---|---|
| `notes` | Always there. `name`, `body`, `sources`, and link kinds that only say how ideas relate: `serves`, `supports`, `contradicts`, `constrains`, `supersedes`, `relates` |
| `formulas` | Always there. `value`, typed in or worked out by a formula, `label` to name what a formula reads, `domain` for a choice, `requires` to say which worlds count, and `macro` for a function of your own. See [docs/expressions.md](docs/expressions.md) |
| `plan` | `task`, `estimate`, `goal`, `question`, `risk`, `unknown`, `done`, `dropped`, `decision`, with `part_of`, `alternative` and `depends_on`. Works out what's needed, completion, blocking, what can start, scope and pressure |
| `schedule` | Dates and slack in every world, from `plan`'s facts. A task's estimate is its duration |

A system also says, in a catalog, what its components mean to someone looking at them: what each is for, which links give the graph its shape, which say what comes first, and what's worth putting on a card. A map is laid out by both kinds, and a tree like the Outline only by shape, because waiting for something isn't being part of it. The app reads that rather than knowing any of it, so a plugin's components get the same treatment. A component no system knows about is kept and shown as it is.

## The plan model

- **`part_of`:** child to parent. All of a node's parts are needed.
- **`decision` and `alternative`:** a decision's options are ordinary nodes, linked to it. Only the option taken is needed.
- **Shared work is just two parents.** Work under two options is `part_of` each, and is safe to do before deciding. Finding that overlap is a big part of the value.
- **`unknown`:** a gap. It says something here isn't specified yet, it can't be done or chosen, and removing it is the claim that it's closed, which has an author and a reason like anything else.
- **`done` is a cell, worked out unless a node says otherwise.** It comes from what's beneath, and reads as finished, still owed, or undefined for something with nothing to finish, which is what a plain note is. Typing over it replaces that, with `true`, `false` or a formula, and removing what was typed brings it back. If what it says disagrees with what's beneath, that's a notice. `dropped` works the same way.
- **Blocking is only inherited through parents in play,** so shared work doesn't wait on a phase this world isn't doing.

## Formulas

A spreadsheet and a puzzle aren't separate systems here. Each is nodes with values, some of them formulas, and a puzzle's solutions are the worlds where a `requires` node is true. Because a formula can read a plan's cells and fill its `estimate`, a plan, its costs and its constraints are one graph: `sum(t in with.task: if(t.need, t.estimate, 0))` is its effort in every world, with shared work counted once, and a `requires` node saying `effort <= 6` rules out the options that can't meet it. Claude writes formulas the same way a person does, as reviewable text in a proposal. A person types them as in a spreadsheet: in the Inspector or the Table, `=` starts a formula and anything else is a value, and a leading `'` means the rest is literal text. See [docs/expressions.md](docs/expressions.md).

A cell is a typed value or a formula, and that's the same whether a person wrote it or a system did. A system can give a component a default formula for every node, which is how dates and `done` work, so what's worked out can be read, overridden and restored like anything else. Some cells stay as compiled rules: what makes worlds, such as `choice`, and for now `need`, the blocking cells and `scope`.

## Views

| Tab | Shows | When |
|---|---|---|
| **Map** | The graph, laid out by the links that give it shape or say what comes first, reading down or across. The Links menu chooses which links shape it, each remembered per workspace in that browser. What a world rules out dims, and nothing moves | Always |
| **Outline** | The same structure, coarse to fine | Always |
| **Table** | Nodes against components. Values and formulas edit in place, as in a spreadsheet, and what just ran is highlighted | Always |
| **Worlds** | Every open choice and what's still possible. Choices with the same options become a grid, and choices with an `at` are placed on a board. A workspace's own [drawings](docs/drawings.md) can be picked instead, and the choice is remembered | If anything's open, or there's a drawing |
| **Decisions** | Each decision, what each option needs, and whether it's holding work up. Deciding shows what it would change first | `plan` |
| **Work** | Tasks by where they stand: can start, depends on a decision, waiting, finished, not needed | `plan` |
| **Timeline** | Dates as a chart, with hatching for how far a date could move | `schedule` |
| **Checks** | Rules that are broken, and notices worth a look | Always |
| **History** | The log as a tree, with every branch. Go to any event, undo any on the way here, and a change after going back starts a new branch | Always |
| **Inbox** | Changes proposed for review, accepted whole or in part | Always |
| **Files** | The workspace's files as they are on disk, read-only, for debugging | Always |

The world bar above them all says what everything's narrowed to.

## Claude, over MCP

One process serves the web UI and an MCP server at `/mcp`, so the UI updates live as Claude works. Claude reads with `get_overview`, `get_node`, `get_tree`, `get_work`, `get_decisions`, `read_world`, `find_checks`, `get_history` and search, most of which take a world, so it can answer a what-if the same way the app does.

It writes with `propose`, whose change-set is checked as if committed, and lands in the inbox with its rationale. A formula with a type error or an unknown input is refused there, with the reason. `apply` writes directly, only if the workspace sets `llm_direct_writes = true`. Ops are the log's five, plus a little to make them easier to write: names for handles, `new:x` for a node that doesn't exist yet, `add_node` and `delete_node`, and `formula` with `labels` to set a formula and say what its names mean as one change. See `crates/recollect-server/src/ops.rs`.

Whichever way a change arrives, the rules are the same: it's all or nothing, and only what it newly breaks is refused, so a hand edit that broke something doesn't block other work.

## Layout

```
crates/
  recollect-graph         The engine: entities, rules, memoisation, worlds, the log. Knows nothing of planning
  recollect-graph-macros  #[component] and #[rule]
  recollect-systems       notes, formulas, plan, schedule, views, and the samples
  recollect-server        The `recollect` binary: workspaces, REST, WebSocket events, MCP
  recollect-kernel        The previous engine, kept only as what the tests compare against
examples/                 The sample workspaces, written by recollect-systems' write_samples
web/                      Vite, React, React Flow and ELK
```

## The samples

The example problems are real workspaces in `examples/`, offered when nothing's open and under the workspace menu. Opening one makes a working copy in `~/.recollect/samples`, so the originals never change, and it can be started again. `tests/samples.rs` checks what's committed still means what it should: an invoice that totals 96, a budget over £200 exactly when `Stay=Hotel`, 92 queens, 288 grids, and the Japanese owning the zebra.

## What was dropped from the first version

The first version had its own model, and these didn't survive the move to the engine:

- **Option scopes on `part_of`, and options as a map on the choice.** Options are nodes, which is what lets them have their own evidence and risks.
- **Robustness as a number.** It assumed paths were independent, which gave wrong answers. Probability is now worked out exactly from the formula, and only where someone gives weights.
- **Status (`live`, `parked`, `dropped`, `superseded`) and per-component states.** `dropped` and `done` are cells, and a `supersedes` link says the rest.
- **Tags, aliases, wikilinks and `created_by`.** A name is a component, and the log says who did what.
- **`merge_nodes`.** It can come back as a proposal that moves links, with no special op.
- **`outcome`, `actionable`, `uncertain`, `principle` and `artifact`, with presets.** Now `goal`, `task` and `question`, with `constrains` for a principle. Confidence on a question went too.
- **Markdown files with frontmatter.** Plain YAML, one file per node, and the log. Older workspaces aren't read, and are left untouched if you try.
- **Client-side what-ifs.** The server narrows by world instead, which is exact.
- **Some gap rules.** Those that needed timestamps or authors on nodes (stale work, ungrounded LLM nodes), and leverage across outcomes, could come back as rules. The rest became notices in `plan`.

## Not built yet

- **Ingesting past sessions, memory and docs,** which is what the first version was heading towards.
- **Merging branches.** Going between them is exact, but nothing brings two together yet. Branches have no names either, only their events.
- **Sharing a workspace between machines.** Files merged by git are fine, and are recorded as an edit, but two logs don't merge.
- **The audit in the app.** The engine can record what a cell read as and notice when code changes what it means, but the app doesn't witness anything yet.
- **A plugin mechanism.** Systems are compiled in. The runtime component and rule APIs are what one would use, and formulas and macros cover much of what people would write one for.
