# Requirements: modelling a design, coarse to fine

**Status:** for discussion. Nothing here is built. It comes from using Recollect to model the architecture of `kept`, a Rust crate for versioning and collaboration, in three layers: the conceptual data model, then data exchange, then implementation interfaces. An agent and a person work on it together over MCP and in the app.

The first draft was discussed, and these were agreed: "in play" moves out of `plan`; relation kinds are nodes; leaning is a component and nothing more; what you're looking at is a formula, which can be named and saved; a read says what it wants and gets it in one batch, in YAML or JSON; and gaps are a standing list rather than warnings on a commit. What's still open is at the end.

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
| `get_node` on kept's Branches | 75 KB, of which 49 KB is history: the first change-set's 343 ops, repeated in full. Its file is about 400 bytes. A small node in the scratch workspace, with three edges and four cells, was 19 KB pretty-printed, also mostly history |
| A plain cell in `get_node` | Said twice: `value`, and `shown` as a list of one case that holds "always", with its probability. A `true` cell has `shown` and no `value` |
| A design decision, with `lean: true` on one option's `alternative` edge | `get_decisions` doesn't show the edge's own cells, so the leaning is invisible. It reports `pressure: idle`, which is about work |
| `get_tree` on a concept whose parts are `relates` edges | Nothing beneath it: only plan edges and `label` arrange |
| Kept's Outline | Every concept is badged Part, options read "Needed when How is ancestry recorded?=Parents by hash", and there's a Work tab |
| Kept's Map at fit | 55 nodes in one row, too small to read |

What already suits a design well: decisions held open as worlds (kept's two open decisions make four worlds, which are the four designs still possible), `ruled_out` with a reason, `supersedes`, `sources`, the log's reasons, and History taking any change back.

## Requirements

### P0: needed before modelling works well

**R1. Structure that isn't a plan.** A concept can be broken into finer concepts with an edge that arranges the Map and the Outline and has no work in it: no `done`, `dropped` or dates, no Work tab, and no "Part" badge. What's beneath an option that isn't taken still dims when a world is pinned. A decision doesn't make everything around it a plan part.

Agreed: a structure system owns the arranging edges, `alternative`, `decision` and `need` ("in play"). `plan` adds `done`, `dropped`, blocking, work and dates on top, and reads `need` rather than defining it. As far as the code shows, `plan` only reads `need` (for `done`, blocking, `actionable`, scope and the schedule), so it loses nothing.

**R2. Reads that say what they want.** One read takes a specification and returns everything that matches in one batch:

```yaml
nodes: { kind: Concept }            # or names, handles, a component, a pattern such as "Delta*"
cells: [name, body, "layer*"]       # or components: [notes]; by default, what was written
edges: { components: [contains, names], cells: true }
worked_out: false                   # true adds evaluated values, and where each came from
history: 0                          # how many recent events, each seq, author, why and op count
world: current intent               # a scenario (R7), or a formula
format: yaml                        # or json
```

- **By default, a node reads as about the size of its file:** what was written, and the name of the node at the other end of each edge. Nothing worked out, no history and no world, unless asked for.
- **Its shape is the file's**, with names beside handles, and what's worked out in a section of its own when asked for, so what someone wrote is never confused with what was worked out. Today's `NodeDetail` isn't shaped like a file, with `outgoing` and `incoming` lists, cells wrapped in `source`, `value` and `shown`, and history attached. So this needs a view struct shaped like the file, which either format then serialises.
- **YAML or JSON is a choice at the MCP boundary.** The views are `Serialize`, the workspace already uses `serde_yaml_ng` for its files, and the engine doesn't care which. YAML is the default for an agent, since it reads and writes the same shape as the files.
- **A value the same in every world is one value.** Where it differs, it's said once, as now, with the worlds it holds in.
- `get_node`, `list_nodes` and `search_nodes` become shorthands for common specifications.

**R3. The overview shows the workspace's own vocabulary.** Beside what systems provide, and before it:

- its own cells, on nodes and on edges, each with how many entities use it and an example value;
- its kinds and relation kinds (R6), each with how many instances and the cells it gives them;
- its tables, and its saved scenarios (R7).

What systems provide can be shorter, or behind a flag, since it's the same in every workspace.

**R4. Instructions for a model, not just a plan or a puzzle.** The server's instructions should be layered: a short core, then a section for each sort of workspace, so an agent reads what applies. The core says that an edge can carry cells of the workspace's own, like a node, and how to write them. The modelling section says how to model a design: kinds for each sort of thing, the structure edge (R1), relation kinds (R6), open decisions with leanings (R7), realising across layers (R8), and reading with a specification (R2). It says which tools are only for plans (`get_work`).

**R5. No silent wrong answers.** `x.in.component` and `x.out.component` either work, going over the edges of `x` rather than `self`, or are refused when written. A `true` or `false` cell has a `value`. Refusals name nodes as well as handles.

### P1: makes modelling good

**R6. Relation kinds: typed edges a workspace defines.** "A header names many parents by hash" is an edge of kind *names*, not `relates` with `role: names`. Agreed: a relation kind is a node, as a kind is. An edge holds a `kind` cell, `{ref: node}`, and takes the cells it doesn't say from that node, as a node takes them from its kind. The relation kind's node says:

- what it means, in its notes;
- whether it arranges the Map and the Outline, and which way up;
- the cells every edge of that kind has, with defaults, such as `many` and `by`;
- optionally, the kinds it goes from and to, checked as a notice.

Then `link {kind: "names"}` works, an edge shows its kind on the Map, and a formula can say `in.names`. That needs the engine to index edges by their kind as it does by component. Since it's a node, defining one is in History, can be undone, and is in the files.

On the way, views and MCP can treat a `role` cell on `relates` as the edge's kind, so kept's existing edges read sensibly before the engine knows about relation kinds.

**R7. Leaning, and looking through a formula.**

*Leaning is data.* A `leaning` component requires `decision` and is per entity, so a kind doesn't pass it on. Its cell holds `{ref: option}`, as `chosen` does, so a decision can only lean one way. A notice, modelled on `chosen_must_be_a_live_option`, says when it leans towards an option that's ruled out or gone. Its reason is in the log. The Decisions tab, the Map and reads show it. No view knows anything else about it.

*What you're looking at is a formula.* Today a view narrows every value by the choices pinned and by `found`, an arbitrary set of worlds that already holds what's ruled out just for looking and pins on decisions whose options depend on others (`Lens` in `views.rs`). A true-or-false formula, evaluated without changing anything (as `try_formula` does now), is combined into `found`. So:

- *Current intent* is `all(d in with.leaning: d.chosen = d.leaning)`. A decision with no leaning isn't constrained, so it narrows the worlds rather than claiming to be one.
- Any other way of looking is another formula: `chose(ancestry, hash) && !chose(signing, every_header)`, or `cost < 4000`.
- What the worlds chip pins and rules out is the same thing, built by clicking.
- Narrowing to something uncertain is supposing it's been found out, as pinning is now, so beliefs are shared out again among the worlds left.

*A formula can be named and saved*, as a scenario, in the workspace rather than the browser, so the person and the agent can both say "read this in *current intent*", and git keeps it. Any read that takes a world takes a scenario's name or a formula.

How this differs from what narrows worlds now:

| | Narrows | Kept in | Changes beliefs |
|---|---|---|---|
| `requires` | your choices, for good | the graph | no |
| `evidence` | what's uncertain, for good, and never a choice of yours | the graph | yes |
| a scenario | both, only while looking | the workspace, beside the graph | yes, as pinning does now |

The engine needs nothing new. The work is in views, a place to keep scenarios, and the `world` parameter.

**R8. Realising across layers.** A built-in `realises` edge says a format or a trait realises a concept, as distinct from being part of it. With it comes a **coverage view**: a row for each concept and a column for each later layer, each cell naming what realises that concept in that layer, such as *Delta wire format* under exchange and *trait DeltaStore* under implementation. Blank cells are the gaps: what's still only an idea. It's the to-do list for going from one layer to the next. Layers stay a cell on kinds, such as `layer`, which the coverage view and the Map's lanes (R13) group by, rather than something the engine knows.

**R9. Reading by kind and by relation.** R2's specification can select by kind, and `get_tree` follows any arranging relation, with the first line of each node's notes. A formula can go over a kind's instances directly, such as `instances(Concept)`, without a label for each.

**R10. A kind's notes aren't its instances'.** A kind's `body` describes the kind. An instance without notes of its own doesn't show it, and isn't found by searching for it. Either `body` is per entity, as `name` and `value` are, or a kind's description is a cell of its own.

**R11. Checks a workspace writes for itself.** A formula that should be true, reported as a notice when it isn't, without ruling out any worlds. `requires` and `evidence` both narrow the worlds, so neither can say "every concept should be realised by something" without making a gap a contradiction. Written on a kind, it runs for each instance and says which fail.

### P2: working together

**R12. What changed since I last looked.** `get_changes {since: seq}` gives the nodes and edges touched, with what was written before and after, in R2's shape. An agent can then see what the person did in the app between turns without rereading the graph.

**R13. A map you can read at this size.** Lanes for a cell's values, such as `layer`, so each layer is a band. Focus on a node and what's within one or two edges. Collapse anything below a chosen depth. Show an edge's kind as its label.

**R14. What's unresolved, as a standing list.** Not warnings on a commit, which only whoever made it sees, and only once. A list in Checks, as notices, that stands until it's fixed and that the person and the agent both see:

- a cell name used only once in the workspace, which is how a typo like `rol` shows;
- a name in a formula with no label, and an edge to a node that isn't there;
- a cell a kind says its instances have, which an instance doesn't say and can't take;
- two nodes with the same name, where something refers to one by name;
- a `role` or relation kind used only once.

It costs nothing on a commit, since it's worked out when asked for.

**R15. Model and prose together.** Export a subtree as markdown, in the shape of a design document: each concept's notes, its parts, its relations, its decisions with their options, leaning and reasons, read through a scenario such as *current intent*. Optionally, `sources` that point at a section of a file, with a notice when that section has changed since. Kept's `DESIGN.md` and its workspace could then be kept in step.

**R16. Decision records.** For each decision: the arguments for and against each option, as `supports` and `contradicts` edges to the options; the leaning; the reasons any option was ruled out; and, once decided, why. Export as an ADR.

**R17. Restructuring as single ops.** Move a node to a new parent, change an edge's kind keeping its cells, and merge two nodes, each one change in History.

### P3: worth considering

- **How fine each part is.** A cell such as `maturity` (sketched, settled, specified), and a view of what's still coarse in each layer.
- **Questions for each other.** A person can mark a node with a question for the agent, and the agent can find open ones, without making it a plan `question` with work in it.
- **What-if structure.** Edges that only hold under an option, such as "Header names parents by hash" only if *Parents by hash* is taken. With R1, putting the edge's source under the option does this without making it work, but it would be clearer if an edge could say it itself.

## What was decided, and why

| Question | Decided | Why |
|---|---|---|
| Structure that isn't a plan | `need` and the arranging edges move into a structure system, under `plan` | "Needed in these worlds" is what a design needs from a plan. A `contains` edge in `notes` wouldn't dim what's under an option, and a profile that hides plan things would only hide the problem |
| Relation kinds | A relation kind is a node, which edges refer to | Kinds already showed that a definition as a node works and stays undoable. Components declared by data would mean rebuilding the registry while a workspace is open |
| Leaning | A component requiring `decision`, holding one option | One by construction, as `chosen` is. `weight` is belief, for what's uncertain, so it can't mean a preference |
| Looking at a design as intended | A formula combined into what a view narrows by, which can be saved by name | General: leaning is one use of it, and the worlds chip's pins are another. Nothing about leaning is built into worlds or views |
| Realising | A built-in `realises` edge | The coverage view and checks need to find it by name |
| Format of reads | YAML or JSON, chosen per read, in the files' shape | The engine doesn't care, and an agent reads and writes one shape |
| Gaps | A standing list of what's unresolved | Everyone sees it, it lasts until fixed, and it costs nothing on a commit |

## Open questions

**Structure**

1. **What is the structure edge called?** If the structure system owns it and `plan` builds on it, it could stay `part_of`, with the badge saying "Part" only where there's work, or be renamed `contains` everywhere, with `part_of` read from older files. One edge for both keeps plans and models one graph.
2. **Can a relation kind arrange?** If *names* is declared as arranging, a model could be laid out by its own edges rather than the structure edge. That's general, but two arranging edges between the same nodes in opposite directions would make a loop on the Map.
3. **Can a relation kind have a kind?** "*names by hash* is a kind of *names*" would follow kinds' lookup. Worth it, or not until it's needed?

**Scenarios**

4. **Where are scenarios kept,** and are saving and changing them in History? A `scenarios.yaml` beside `recollect.toml` keeps them out of the graph. In the graph, as nodes with a formula, they'd have History, undo and reasons for free, and the agent could write them with `apply`. That makes them data, which is what you flinched at for leaning, but they'd be data about looking rather than about the design.
5. **What does a scenario do where its formula is undefined?** Probably it rules nothing out there, so a scenario never hides a world because of a gap.
6. **Is the worlds chip's state an unsaved scenario?** If so, "save this view" is one click, and a saved scenario can be opened, changed by clicking, and saved again.
7. **Can a scenario mention what's uncertain?** Supposing something's found out changes beliefs, which is right for "what if the backup is old", but could surprise someone who only meant to narrow their own choices.

**Reads**

8. **Does the read specification replace `get_node`, `list_nodes`, `search_nodes` and `get_tree`,** or sit beside them as the general form? Fewer tools are easier for an agent to choose between, but a plain `get_node` is the obvious first call.
9. **Names or handles?** Handles are 36 characters, and most of the size of a small read. Names are readable but not unique. Short handles, such as the last eight characters where they're unique in the workspace, would help both.
10. **Should the app use the same read specification,** so there's one read path rather than the full `View` and the MCP reads?

**Leaning and decisions**

11. **Does a leaning need a reason cell,** or is the reason in the log enough? The log's reason is lost between sessions unless git keeps it, so a short `why` beside the leaning may be worth having.
12. **Should open decisions in a model make worlds at all?** Two decisions are four worlds, which is useful: they're the four designs still possible. Twenty are a million, and the worlds chip would say so. *Current intent* makes that manageable, but does a model want the count shown at all?

**Kept**

13. **One workspace, or one per layer?** One keeps `realises` inside it and lets the Map show lanes. This assumes one.
14. **When is kept's workspace rewritten,** and by which session? Once R1, R6 and R7 exist: concepts as instances of `Concept`, the structure edge in place of plan parts, relation kinds in place of `relates` with `role`, and leanings on its two decisions.

## Suggested order

1. R5, R2, R3, R4: correctness and reads, with no model changes.
2. R1 and R10: structure without work, and kinds that don't leak notes.
3. R7: leaning, then views narrowing by a formula, then saved scenarios.
4. R8 and R6, starting with views reading `role`, then relation kinds as nodes.
5. R14, R11 and R12, then the rest, as kept's work shows which matter.
