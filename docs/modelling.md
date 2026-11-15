# Requirements: modelling a design, coarse to fine

**Status:** for discussion. Nothing here is built. It comes from using Recollect to model the architecture of `kept`, a Rust crate for versioning and collaboration, in three layers: the conceptual data model, then data exchange, then implementation interfaces. An agent and a person work on it together over MCP and in the app.

The first draft was discussed, and these were agreed: "in play" moves out of `plan`; relation kinds are nodes; leaning is a component and nothing more; what you're looking at is a formula; a read says what it wants and gets it in one batch, in YAML or JSON; MCP is redesigned with no overlapping tools; and gaps are a standing list rather than warnings on a commit. Saved scenarios are left out for now.

**No backward compatibility.** Nobody uses MCP yet, and the examples are the only workspaces, so formats, names and tools change freely, and the examples are changed with the engine. Nothing reads an old form.

What's still open is at the end.

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
edges: { components: [part_of, names], cells: true }
worked_out: false                   # true adds evaluated values, and where each came from
history: 0                          # how many recent events, each seq, author, why and op count
where: "all(d in with.leaning: d.chosen = d.leaning)"   # narrows the worlds (R7)
format: yaml                        # or json
```

- **By default, a node reads as about the size of its file:** what was written, and the name of the node at the other end of each edge. Nothing worked out, no history and no world, unless asked for. Size matters less than shape: the aim is that nothing comes back that wasn't asked for.
- **Its shape is the file's**, with what's worked out in a section of its own when asked for, so what someone wrote is never confused with what was worked out. Today's `NodeDetail` isn't shaped like a file, with `outgoing` and `incoming` lists, cells wrapped in `source`, `value` and `shown`, and history attached. So this needs a view struct shaped like the file, which either format then serialises.
- **Every reference is a full handle with the name beside it**, such as `to: 01a1dce9-2879-7010-b5c0-5126c9abb977` and `to_name: Deltas`. The handle is what `apply` takes, so an agent copies it rather than looking it up; the name is what it reasons with. Short handles would be one more thing to resolve, and names alone aren't unique.
- **YAML or JSON is a choice at the MCP boundary.** The views are `Serialize`, the workspace already uses `serde_yaml_ng` for its files, and the engine doesn't care which. YAML is the default for an agent, since it reads and writes the same shape as the files.
- **A value the same in every world is one value.** Where it differs, it's said once, as now, with the worlds it holds in.
- **It replaces the reading tools.** MCP is redesigned around it with no overlapping tools, and what doesn't fit is deleted: `get_node`, `list_nodes`, `search_nodes`, `get_tree`, `read_world`, `get_decisions` and `get_work` all become specifications of one `read`. The proposed set is in the open questions.

**R3. The overview shows the workspace's own vocabulary.** Beside what systems provide, and before it:

- its own cells, on nodes and on edges, each with how many entities use it and an example value;
- its kinds and relation kinds (R6), each with how many instances and the cells it gives them;
- its tables.

What systems provide can be shorter, or behind a flag, since it's the same in every workspace.

**R4. Instructions for a model, not just a plan or a puzzle.** The server's instructions should be layered: a short core, then a section for each sort of workspace, so an agent reads what applies. The core says that an edge can carry cells of the workspace's own, like a node, and how to write them. The modelling section says how to model a design: kinds for each sort of thing, the structure edge (R1), relation kinds (R6), open decisions with leanings (R7), realising across layers (R8), and reading with a specification (R2). It says which specifications are only for plans, such as work and blocking.

**R5. No silent wrong answers.** `x.in.component` and `x.out.component` either work, going over the edges of `x` rather than `self`, or are refused when written. A `true` or `false` cell has a `value`. Refusals name nodes as well as handles.

### P1: makes modelling good

**R6. Relation kinds: typed edges a workspace defines.** "A header names many parents by hash" is an edge of kind *names*, not `relates` with `role: names`. Agreed: a relation kind is a node, as a kind is. An edge holds a `kind` cell, `{ref: node}`, and takes the cells it doesn't say from that node, as a node takes them from its kind. The relation kind's node says:

- what it means, in its notes;
- whether it arranges the Map and the Outline, and which way up;
- the cells every edge of that kind has, with defaults, such as `many` and `by`;
- optionally, the kinds it goes from and to, checked as a notice.

Then `link {kind: "names"}` works, an edge shows its kind on the Map, and a formula can say `in.names`. That needs the engine to index edges by their kind as it does by component. Since it's a node, defining one is in History, can be undone, and is in the files.

There's no interim step reading `role` on `relates`: with no backward compatibility, kept's edges are rewritten once relation kinds exist.

**R7. Leaning, and looking through a formula.**

*Leaning is data.* A `leaning` component requires `decision` and is per entity, so a kind doesn't pass it on. Its cell holds `{ref: option}`, as `chosen` does, so a decision can only lean one way. A notice, modelled on `chosen_must_be_a_live_option`, says when it leans towards an option that's ruled out or gone. Its reason is in the log. The Decisions tab, the Map and reads show it. No view knows anything else about it.

*What you're looking at is a formula.* Today a view narrows every value by the choices pinned and by `found`, an arbitrary set of worlds that already holds what's ruled out just for looking and pins on decisions whose options depend on others (`Lens` in `views.rs`). A true-or-false formula, evaluated without changing anything (as `try_formula` does now), is combined into `found`. So:

- *Current intent* is `all(d in with.leaning: d.chosen = d.leaning)`. A decision with no leaning isn't constrained, so it narrows the worlds rather than claiming to be one.
- Any other way of looking is another formula: `chose(ancestry, hash) && !chose(signing, every_header)`, or `cost < 4000`.
- What the worlds chip pins and rules out is the same thing, built by clicking.
- Narrowing to something uncertain is supposing it's been found out, as pinning is now, so beliefs are shared out again among the worlds left.

Any read takes such a formula as `where` (R2), and the app can take one too. Saving formulas by name, as scenarios, is left out for now: a leaning and a formula are enough to begin with.

How this differs from what narrows worlds now:

| | Narrows | Kept in | Changes beliefs |
|---|---|---|---|
| `requires` | your choices, for good | the graph | no |
| `evidence` | what's uncertain, for good, and never a choice of yours | the graph | yes |
| a `where` formula | both, only while looking | the read, or the view | yes, as pinning does now |

The engine needs nothing new. The work is in views and the `where` parameter.

**R8. Realising across layers.** A built-in `realises` edge says a format or a trait realises a concept, as distinct from being part of it. With it comes a **coverage view**: a row for each concept and a column for each later layer, each cell naming what realises that concept in that layer, such as *Delta wire format* under exchange and *trait DeltaStore* under implementation. Blank cells are the gaps: what's still only an idea. It's the to-do list for going from one layer to the next. Layers stay a cell on kinds, such as `layer`, which the coverage view and the Map's lanes (R13) group by, rather than something the engine knows.

**R9. Reading by kind and by relation.** R2's specification can select by kind, and a read as a tree follows any arranging relation, with the first line of each node's notes. A formula can go over a kind's instances directly, such as `instances(Concept)`, without a label for each.

**R10. A kind's notes aren't its instances'.** A kind's `body` describes the kind. An instance without notes of its own doesn't show it, and isn't found by searching for it. Either `body` is per entity, as `name` and `value` are, or a kind's description is a cell of its own.

**R11. Checks a workspace writes for itself.** A formula that should be true, reported as a notice when it isn't, without ruling out any worlds. `requires` and `evidence` both narrow the worlds, so neither can say "every concept should be realised by something" without making a gap a contradiction. Written on a kind, it runs for each instance and says which fail.

### P2: working together

**R12. What changed since I last looked.** `changes {since: seq}` gives the nodes and edges touched, with what was written before and after, in R2's shape. An agent can then see what the person did in the app between turns without rereading the graph.

**R13. A map you can read at this size.** Lanes for a cell's values, such as `layer`, so each layer is a band. Focus on a node and what's within one or two edges. Collapse anything below a chosen depth. Show an edge's kind as its label.

**R14. What's unresolved, as a standing list.** Not warnings on a commit, which only whoever made it sees, and only once. A list in Checks, as notices, that stands until it's fixed and that the person and the agent both see:

- a cell name used only once in the workspace, which is how a typo like `rol` shows;
- a name in a formula with no label, and an edge to a node that isn't there;
- a cell a kind says its instances have, which an instance doesn't say and can't take;
- two nodes with the same name, where something refers to one by name;
- a `role` or relation kind used only once.

It costs nothing on a commit, since it's worked out when asked for.

**R15. Model and prose together.** Export a subtree as markdown, in the shape of a design document: each concept's notes, its parts, its relations, its decisions with their options, leaning and reasons, read through a formula such as *current intent*. Optionally, `sources` that point at a section of a file, with a notice when that section has changed since. Kept's `DESIGN.md` and its workspace could then be kept in step.

**R16. Decision records.** For each decision: the arguments for and against each option, as `supports` and `contradicts` edges to the options; the leaning; the reasons any option was ruled out; and, once decided, why. Export as an ADR.

**R17. Restructuring as single ops.** Move a node to a new parent, change an edge's kind keeping its cells, and merge two nodes, each one change in History.

### P3: worth considering

- **How fine each part is.** A cell such as `maturity` (sketched, settled, specified), and a view of what's still coarse in each layer.
- **Questions for each other.** A person can mark a node with a question for the agent, and the agent can find open ones, without making it a plan `question` with work in it.
- **What-if structure.** Edges that only hold under an option, such as "Header names parents by hash" only if *Parents by hash* is taken. With R1, putting the edge's source under the option does this without making it work, but it would be clearer if an edge could say it itself.

## What was decided, and why

| Question | Decided | Why |
|---|---|---|
| Structure that isn't a plan | `need` and the arranging edges move into a structure system, under `plan` | "Needed in these worlds" is what a design needs from a plan. A new edge in `notes` wouldn't dim what's under an option, and a profile that hides plan things would only hide the problem |
| What the structure edge is called | `part_of`, kept | See below |
| Relation kinds | A relation kind is a node, which edges refer to | Kinds already showed that a definition as a node works and stays undoable. Components declared by data would mean rebuilding the registry while a workspace is open |
| Leaning | A component requiring `decision`, holding one option | One by construction, as `chosen` is. `weight` is belief, for what's uncertain, so it can't mean a preference |
| Looking at a design as intended | A formula combined into what a view narrows by | General: leaning is one use of it, and the worlds chip's pins are another. Nothing about leaning is built into worlds or views |
| Saved scenarios | Not now | A leaning and a formula are enough to start with |
| Realising | A built-in `realises` edge | The coverage view and checks need to find it by name |
| Format of reads | YAML or JSON, chosen per read, in the files' shape | The engine doesn't care, and an agent reads and writes one shape |
| References in reads | Full handles, with names beside them | Size isn't the aim. A handle is what `apply` takes, so it's copied, never looked up |
| MCP tools | Redesigned, with no overlapping tools, and what doesn't fit deleted | Nobody uses MCP yet |
| Compatibility | None. The examples change with the engine | They're the only workspaces |
| Gaps | A standing list of what's unresolved | Everyone sees it, it lasts until fixed, and it costs nothing on a commit |

**Why `part_of` and not `contains`.** The first draft suggested `contains`, but only to get away from plans, and that isn't needed. What makes kept's concepts plan parts is that `part_of` today requires the `part` component at both ends, and `part` carries `done`, `dropped` and blocking. Once the structure system owns `part_of` and `need`, the edge requires only that, and `part` is what `plan` adds for work. And `part_of` points the right way for the files: it goes from the child to its parent, so each node's file says where it sits, a node has one parent, and moving it changes one file. `contains` would go from the parent, putting every child in the parent's file.

## Open questions

**Structure**

Each has a suggested answer and the reason for it.

**MCP**

1. **Which tools are there?** Suggested: five.
   - `overview`: the vocabulary (R3), and the instructions that apply (R4).
   - `read`: the specification (R2), covering what `get_node`, `list_nodes`, `search_nodes`, `get_tree`, `read_world`, `get_decisions` and `get_work` do now.
   - `apply`: ops, as now.
   - `changes`: history, and what changed since a seq (R12), replacing `get_history`.
   - `checks`: what's unresolved and what fails (R11, R14), replacing `find_checks`.

   *Why:* each answers a different question (what words exist, what's there, change it, what happened, what's wrong), so an agent never has two ways to do one thing. Decisions and work are reads of particular components with what's worked out, so they don't need tools of their own; a few named specifications in the instructions, such as "decisions" or "work", cover the common cases.
2. **Should the app read through the same specification?** Suggested: yes, inside the engine, but not yet in the app's API. *Why:* one read path means the app and the agent can't disagree about a value. But the app wants the whole graph at once for the Map, so it gains nothing until it reads less; the engine function can be shared now and the app moved onto it later.

**Structure**

3. **Can a relation kind arrange the Map and Outline?** Suggested: yes, if its node says so, with a notice when two arranging edges make a loop. *Why:* a data model is naturally laid out by its own relations, such as a header beneath what it names, and `ComponentDef.arranges` already lets more than one edge arrange.
4. **Can a relation kind have a kind?** Suggested: not yet. *Why:* nothing in kept needs "*names by hash* is a kind of *names*" so far, and adding it later wouldn't change any file already written.

**Looking through a formula**

5. **What does a `where` formula do in a world where it's undefined,** such as a cell missing in that world? Suggested: it keeps the world and says it couldn't decide there. *Why:* dropping it would hide a design because of a gap in the data, which is a silent wrong answer (R5).
6. **Can a `where` formula mention something uncertain?** Suggested: yes, and the read says it's supposing that. *Why:* "what if the backup is old" is a useful question and pinning already works this way; saying so is what stops it surprising someone who only meant to narrow their own choices.
7. **Is the worlds chip a formula editor?** Suggested: yes, the chip builds a formula, and it can show and take one as text. *Why:* then clicking and typing are one mechanism, and what someone sees in the app can be pasted into a read.

**Leaning and decisions**

8. **Does a leaning need its own `why` cell?** Suggested: yes, optional. *Why:* a leaning is the choice most likely to be revisited, and the log's reason isn't in the files, so it's lost unless the history is kept; a `why` beside the leaning travels with it and shows in reads.
9. **Should a model show how many worlds there are?** Suggested: show how many open decisions there are, and the worlds only when asked. *Why:* "four designs still possible" helps, but "a million worlds" for twenty decisions alarms without helping; open decisions is the number someone acts on.

**Kept**

10. **One workspace, or one per layer?** Suggested: one. *Why:* `realises` crosses layers, and there are no references between workspaces; lanes (R13) give each layer its own band.
11. **When is kept's workspace rewritten, and by which session?** Suggested: once R1, R6 and R7 are built, by the kept session that owns its design, not the one changing the engine. *Why:* the rewrite is design work (which concepts, which relations, which leanings), and with no backward compatibility it has to happen in the same step as those changes land, or kept's workspace stops opening.

## Suggested order

1. R5, R2, R3, R4: correctness, and MCP redesigned around one read, with no model changes.
2. R1 and R10: structure without work, and kinds that don't leak notes.
3. R7: leaning, then views narrowing by a formula.
4. R6 and R8: relation kinds as nodes, then `realises` and the coverage view.
5. R14, R11 and R12, then the rest, as kept's work shows which matter.
