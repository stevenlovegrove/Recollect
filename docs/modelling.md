# Requirements: modelling a design, coarse to fine

**Status:** for discussion. Nothing here is built. It comes from using Recollect to model the architecture of `kept`, a Rust crate for versioning and collaboration, in three layers: the conceptual data model, then data exchange, then implementation interfaces. An agent and a person work on it together over MCP and in the app.

## In short

Recollect's engine can already hold a design model. Any node or edge can carry cells of the workspace's own, kinds give nodes shared cells, and a formula on a kind can count typed edges. Three things get in the way:

1. **Its only structure is a plan's.** Breaking a concept into finer ones makes it a plan `part`, with `need`, `done` and a Work tab that mean nothing for a model.
2. **The workspace's own vocabulary can't be seen.** Its own cells, its kinds and its typed edges aren't in the overview, can't be listed or filtered, and can't be defined as anything more than a convention.
3. **MCP reads are too big and too plan-shaped** for an agent to work in short turns.

The requirements below are ranked by how much each unblocks. P0 is needed before modelling works well; P1 makes it good; P2 and P3 are ideas for working together.

## What was tried

On a scratch workspace, over MCP, with the server built from `main` at `dc50603`, and by reading kept's workspace without changing it.

| Tried | What happened |
|---|---|
| A component of the workspace's own, `concept` | Refused: `unknown_component: 01a1… has concept, which no system provides`. The message names a handle, not the node |
| Kinds `Concept`, `Format` and `Interface`, with `layer` and `status` cells, and instances by `is_a` | Works. Instances take `layer` from their kind |
| `relates` edges with cells `role`, `many` and `by` | Works, and `get_node` shows them |
| `realised_by = count(x in in.relates: x.via.role = "realises")` on `Concept` | Works: Delta is realised by 2, Header by 0. Folding over edges, filtered by an edge's cell, is already possible |
| `count(c in concept.in.is_a: …)`, to go over a kind's instances | **Accepted, and wrong.** `in` only works at the start of a path, so this reads as a cell path and counts 1, not 3. It should have been refused |
| Going over a kind's instances by labelling each one | Works (2 unrealised), but each new instance needs a new label |
| An instance with no notes of its own | It shows its kind's notes as its own, so searching for "trait" finds `trait DeltaStore` through `Interface`'s description |
| `get_overview` | Lists every system's 38 components and 24 cells (18 KB of 25 KB). None of the workspace's own cells (`layer`, `status`, `role`, `many`) and no kinds |
| `list_nodes {component: "is_a"}` | Empty: it filters by a node's components, and there's no way to filter by kind |
| `get_node` on kept's Branches | 75 KB, of which 49 KB is history: the first change-set's 343 ops, repeated in full. A small node in the scratch workspace, with three edges and four cells, was 19 KB pretty-printed, also mostly history |
| A plain cell in `get_node` | Said twice: `value`, and `shown` as a list of one case that holds "always", with its probability. A `true` cell has `shown` and no `value` |
| A design decision, with `lean: true` on one option's `alternative` edge | `get_decisions` doesn't show the edge's own cells, so the leaning is invisible. It reports `pressure: idle`, which is about work |
| `get_tree` on a concept whose parts are `relates` edges | Nothing beneath it: only plan edges and `label` arrange |
| Kept's Outline | Every concept is badged Part, options read "Needed when How is ancestry recorded?=Parents by hash", and there's a Work tab |
| Kept's Map at fit | 55 nodes in one row, too small to read |

What already suits a design well: decisions held open as worlds (kept's two open decisions make four worlds, which are the four designs still possible), `ruled_out` with a reason, `supersedes`, `sources`, the log's reasons, and History taking any change back.

## Requirements

### P0: needed before modelling works well

**R1. Structure that isn't a plan.** A concept can be broken into finer concepts with an edge that arranges the Map and the Outline and has no work in it: no `done`, `dropped` or dates, no Work tab, and no "Part" badge.

- What's beneath an option that isn't taken should still dim when a world is pinned. Today that comes from `need`, which is `plan`'s. See *Options* below.
- A decision should be possible without making everything around it a plan part.

**R2. A `get_node` an agent can afford.** By default:

- history is short: the last few events that touched it, each with `seq`, `author`, `why` and how many ops, and a way to ask for an event's ops;
- a cell that's the same in every world is one value, not a value and a `shown`;
- defaults and worked-out cells nobody wrote are left out, or there's an option to leave them out (`cells: "written" | "all"`);
- each edge is one line: the other node's name, the components, and its cells.

The kept node that's 75 KB today should be under 2 KB.

**R3. The overview shows the workspace's own vocabulary.** Beside what systems provide, and before it:

- its own cells, on nodes and on edges, each with how many entities use it and an example value;
- its kinds, each with how many instances and the cells it gives them;
- its tables;
- its relation kinds, once R6 is built.

What systems provide can be shorter, or behind a flag, since it's the same in every workspace.

**R4. Instructions for a model, not just a plan or a puzzle.** The server's instructions should say, near the top:

- that an edge can carry cells of the workspace's own, like a node, and how to write them (`link` with `cells`, and `set` with `from` and `to`);
- how to model a design: kinds for each sort of thing, the structure edge from R1, typed edges, open decisions as decisions, a leaning (R7), and realising across layers (R8);
- which tools to use for a model (`get_tree`, `get_node` with short output) and which are only for plans (`get_work`).

Instructions should be layered: a short core, then a section for each sort of workspace, so an agent reads what applies.

**R5. No silent wrong answers.** `x.in.component` and `x.out.component` should either work (going over the edges of `x`, not `self`) or be refused when written. A `true` or `false` cell should have a `value`. Refusals should name nodes as well as handles.

### P1: makes modelling good

**R6. Relation kinds: typed edges a workspace defines.** "A header names many parents by hash" should be an edge of kind *names*, not `relates` with `role: names`. A relation kind says:

- its name and what it means;
- whether it arranges the Map and Outline, and which way up;
- the cells every edge of that kind has, with defaults, such as `many` and `by`;
- optionally, the kinds it goes from and to, checked as a notice.

Then `link {component: "names"}` works, edges show their kind on the Map, and a formula can say `in.names`. See *Options* below for how.

**R7. Leaning towards an option.** A decision can say which option it leans towards, and why, without deciding. It's shown on the Decisions tab, the Map and in `get_decisions`. A single "as leaning" world pins every decision to its leaning, so the design as currently intended can be read in one view, which is what a design document describes.

**R8. Realising across layers.** An edge that says a format or a trait realises a concept, distinct from being part of it. With it:

- a coverage view: each concept, and what realises it in each later layer, with the gaps;
- the Map can lay the layers out as lanes (R13);
- a check can report a concept nothing realises (R11).

**R9. Reading by kind and by relation.** `list_nodes` takes a `kind`, and returns names with each node's written cells in one line. `get_tree` follows any arranging relation, includes the first line of each node's notes, and can list a node's other edges with their kinds and cells. A formula can go over a kind's instances directly, such as `instances(Concept)` or `Concept.instances`, without a label for each.

**R10. A kind's notes aren't its instances'.** A kind's `body` describes the kind. An instance without notes of its own shouldn't show it, or be found by searching for it. Either `body` is per entity, as `name` and `value` are, or a kind's description is a cell of its own.

**R11. Checks a workspace writes for itself.** A formula that should be true, reported as a notice when it isn't, without ruling out any worlds. `requires` and `evidence` both narrow the worlds, so neither can say "every concept should be realised by something" without making a gap a contradiction. Such a check could be per instance, written on a kind, so it reports which instance fails.

### P2: working together

**R12. What changed since I last looked.** `get_history {since: seq}` with short events, and `get_changes {since: seq}`, which gives the nodes and edges touched, with their cells before and after. An agent can then see what the person did in the app between turns, without rereading the graph.

**R13. A map you can read at this size.** Lanes for a cell's values, such as `layer`, so each layer is a band. Focus on a node and what's within one or two edges. Collapse anything below a chosen depth. Show an edge's kind, or its `role` for now, as its label.

**R14. Warnings with a commit.** `apply` returns warnings that don't refuse it:

- a cell name used for the first time in the workspace, which is how a typo like `rol` would show;
- a new node with the same name as an existing one;
- an instance missing a cell its kind says every instance should have.

**R15. Model and prose together.** Export a subtree as markdown, in the shape of a design document: each concept's notes, its parts, its relations, its decisions with their options, leaning and reasons. Optionally, `sources` that point at a section of a file, and a notice when that section has changed since. Kept's `DESIGN.md` and its workspace could then be kept in step.

**R16. Decision records.** For each decision: the arguments for and against each option, as `supports` and `contradicts` edges to the options; the leaning; the reasons any option was ruled out; and, once decided, why. Export as an ADR.

**R17. Restructuring as single ops.** Move a node to a new parent, change an edge's kind keeping its cells, and merge two nodes, each one change in History.

### P3: worth considering

- **How fine each part is.** A cell such as `maturity` (sketched, settled, specified), and a view of what's still coarse in each layer.
- **Questions for each other.** A person can mark a node with a question for the agent, and the agent can find open ones, without making it a plan `question` with work in it.
- **What-if structure.** Edges that only hold under an option, such as "Header names parents by hash" only if *Parents by hash* is taken, so pinning the other option shows the model as it would then be. Today that's only possible by putting the edge's source under the option as a plan part.

## Options

### Structure that isn't a plan (R1)

| Option | What it means | For | Against |
|---|---|---|---|
| **A. A `contains` edge in `notes`** | Arranges, and nothing else | Small. Plan stays as it is | Pinning a world doesn't dim what's under an option, since `need` is `plan`'s |
| **B. Split "in play" out of `plan`** | A structure system owns `part_of`, `alternative`, `decision` and `need`. `plan` adds `done`, `dropped`, blocking and work on top | What's under an option dims, in a model or a plan. The Work tab and `done` only appear with work | A real refactor of `plan.rs`, and the samples and tests that lean on it |
| **C. Profiles** | `recollect.toml` says a workspace is a model, and views hide and rename plan things | Cosmetic, so quick | `need` and `done` are still worked out and still in MCP output. It hides the problem rather than fixing it |

Recommended: **B**, with A's edge name. "Needed in these worlds" is what a design needs from a plan, and the rest of a plan is what it doesn't.

### How a workspace defines a relation kind (R6)

| Option | What it means | For | Against |
|---|---|---|---|
| **A. A relation kind is a node** | An edge holds a `kind` cell, `{ref: node}`, as `chosen` holds what a choice lands on. The edge takes cells from that node as a node does from its kind. The node says `arranges`, `reversed`, `from` and `to` as cells | The same as kinds for nodes: data, in the graph, with History and undo. No change to the registry | Edges look up a cell differently from nodes. `in.names` needs the engine to index edges by their kind. Two ways to say what an edge is |
| **B. Components declared by data** | A node with a `relation` component declares a component that the registry adds when the workspace opens | One way to say what an edge is, the same as built-in edges. Map and Outline already read `arranges` from component definitions | The registry is fixed at start today. Changing a definition means rebuilding the registry, and memos keyed on it. Opens the question of node components declared by data too, which kinds already cover |
| **C. Keep `relates` and `role`** | Views and MCP treat a `role` cell on `relates` as the edge's kind | No engine change | A convention the engine doesn't know: nothing checks a role's cells, and `role` is spelt by hand each time |

Recommended: **A** for now, since kinds proved that "a definition is a node" works and stays undoable. C is a cheap step on the way, since views would read the same thing either way.

### Leaning (R7)

| Option | For | Against |
|---|---|---|
| **A. A `lean` cell on an `alternative` edge** | Next to `weight` and `ruled_out` | Two options can both lean. Needs a check |
| **B. A `leaning` cell on the decision, holding `{ref: option}`, beside `chosen`** | Only one by construction, as `chosen` is. The "as leaning" world is easy: pin each decision to its leaning | One more cell on `decision` |
| **C. Use `weight`** | Already there | `weight` is belief, for what's uncertain. A choice that's someone's to make isn't believed, and the app says so |

Recommended: **B**, with its reason in the log and, if wanted, a `why` cell beside it.

### Realising across layers (R8)

| Option | For | Against |
|---|---|---|
| **A. A built-in `realises` edge in `notes`** | Coverage views and checks can rely on its name | One more built-in edge |
| **B. A relation kind (R6) marked as realising** | No new built-in | Views wait for R6, and need a flag to find it |

Recommended: **A**. Layers themselves stay a cell on kinds, such as `layer`, which R13's lanes can group by, rather than something the engine knows.

## Open questions

1. **One workspace, or one per layer?** One keeps `realises` edges inside it and lets the Map show lanes. Separate ones would need links between workspaces. This assumes one.
2. **Should open decisions in a model make worlds at all?** Four worlds for two decisions is useful: they're the four designs still possible. But a model with twenty open decisions has a million, and the worlds chip would say so. Does a model want worlds, or only the leaning (R7) and pinning by hand?
3. **Is B for R1 worth it now,** or is A enough until a model needs what's beneath an option to dim?
4. **Should relation kinds check their ends?** "Names goes from Header to Delta" could be a notice, or nothing.
5. **How strict should R14's warnings be** before they're noise: every new cell name, or only one close to an existing name?
6. **Should MCP output be text rather than JSON** for the reads an agent does most (`get_tree`, `get_node`, `list_nodes`)? An indented outline is a fraction of the size and easier to read.
7. **What happens to kept's workspace?** Once R1, R6 and R7 exist, it would be rewritten: concepts as instances of `Concept`, `contains` in place of `part_of`, typed edges in place of `relates` with `role`, and leanings on its two decisions.

## Suggested order

1. R2, R3, R4, R5: MCP and correctness, no model changes. Each is small.
2. R1 (option A first), R7, R8, R10: what a model needs, mostly in `notes` and `plan`.
3. R9 and R6, starting with C so views read `role`, then A.
4. R11 to R14, then the rest, as kept's work shows which matter.
