# Implementation plan: modelling a design

This plan builds what [modelling.md](modelling.md) agreed. It's in five phases. Each phase leaves the
workspace building, its tests passing, and the examples regenerated, so work can stop after any of them.
Within a phase, each step is one or a few small commits.

## Ground rules

- **No backward compatibility.** When a name, a file shape or a tool changes, it changes everywhere in
  the same commit. The examples are written by `samples.rs`, so a change to them is a change there,
  followed by deleting `examples/*` and running
  `cargo run -p recollect-systems --example write_samples -- examples`. The diff of `examples/` is then
  the review of what the change did to real workspaces.
- **One read path.** `dispatch.rs` is what both the server's `/api/call/{method}` and the wasm worker
  call, so anything the app reads goes through it. MCP's `read` and the app's `view` share one engine
  function in a new `recollect-core/src/read.rs`.
- **Tests come first where behaviour changes.** Each step below names the test files it touches. When a
  test fails, it's reported, not weakened.
- **Checked by** `cargo test --workspace`, `cargo clippy --workspace`, and, for a step that touches
  `web/`, `npm run build` in `web/` and a look at the app on an example.

## Phase 1: correct answers, and MCP redesigned (R5, R2, R3, R4, R12)

This phase changes no model. It makes reads right, then replaces the MCP tools.

### 1.1 No silent wrong answers (R5)

- **`in`, `out` and `with` after the start of a path.** Today `expr.rs` (around `path_from`, and the
  group check at line ~543) only treats them as steps at the start, so `concept.in.is_a` is read as a
  cell path. Make them work anywhere: when the evaluator in `formulas.rs` (`fn path`, line ~840)
  reaches `in` or `out` after a node binding, it follows that node's edges rather than `cx`'s. If that
  turns out to be deep in the evaluator, the first commit refuses the formula when it's written instead,
  with a message saying where `in` may go, and the second makes it work.
  Tests: `tests/expr.rs`, `count(c in kind.in.is_a: true)` is 3 for three instances.
- **A true-or-false cell has a `value`.** In `views.rs`, `CellView` gets `value` for `Shown::Bool` when
  it holds in every world in play.
- **Refusals name nodes.** `error_text` (`views.rs:473`) and the refusals `ops.rs` builds name a node as
  `Name (handle)`, using `label_of` from `recollect-systems/src/lib.rs`.
  Tests: `crates/recollect-server/src/tests.rs` for a refused `apply`.

### 1.2 The read specification (R2)

New `recollect-core/src/read.rs`, exposed through `dispatch.rs` as `"read"`.

- **`ReadSpec`** (serde, both YAML and JSON):
  - `nodes`: handles, names, `kind`, `component`, a glob pattern on the name, `text` (what
    `search_nodes` did), or `tree: {root, depth, edges}` (what `get_tree` did). Several of them mean
    *and*.
  - `cells`: a list of names or globs, or `components: [...]`. By default, what was written.
  - `edges`: which components, which way, and whether their cells come too.
  - `worked_out`: adds each cell's value in the worlds in play, and where it came from, in a section of
    its own.
  - `history`: how many recent events per node, as seq, author, why and op count, never the ops.
  - `where`: a formula (1.3).
  - `world`: pins, as `World` today, for the app and for what-ifs.
- **The output is the file's shape.** Each node is a `SavedNode` (`recollect-graph/src/saved.rs`),
  which is exactly what's in its file, plus `name` beside every handle it refers to (`to_name` on each
  link), plus the optional `worked_out` and `history`. Reusing `SavedNode` is what guarantees "the
  shape of the file". Incoming edges, when asked for, are listed as the links they are in other nodes'
  files, under `incoming`.
- **A value the same in every world is one value.** The `worked_out` section says it once. Where it
  differs, it lists the worlds it holds in, as `ShownView` does now.
- **Format.** `format: yaml | json` is applied in the MCP layer only. The engine returns a
  `Serialize` struct.
- **Replace and delete.** `views::node`, `list`, `search` and `tree`, with `NodeDetail`, `EdgeBrief`,
  `Brief` and `TreeNode`, are deleted once MCP uses `read`. `planning`, the decisions view and `work`
  become specifications: `nodes: {component: decision}` with `worked_out`, and `nodes: {component:
  work}`, with what's worked out by plan (pressure, standing).

Tests: a new `crates/recollect-core/src/read/tests.rs`. Kept's Branches-shaped node reads as no larger
than its file plus names; `history: 0` has no history; JSON and YAML say the same thing; a selection by
kind finds instances.

### 1.3 Looking through a formula

- **`Lens::new`** (`views.rs:579`) takes an optional `where` formula. It's compiled and evaluated on the
  whole (`Entity::whole()`) without changing the graph, as `try_formula` does. Its truth is a `Bool` over
  the worlds, and it's combined into `found`: `found & (truth | undefined)`. A world where it's undefined
  is kept, and the read counts such worlds and says so (`undecided: n`).
- **Supposing.** If the formula's `Bool` mentions a variable for which `is_chance` holds, the read says
  `supposing: [...]` with those variables' names.
- **Errors.** A formula that doesn't compile is refused, with the position, as a cell's formula is.
- `view` (the app's read) takes `where` too, so the app can use it in Phase 3.

Tests: `views` tests or `read/tests.rs`: a formula on one decision halves four worlds to two; an
undefined formula keeps every world and reports them; a formula about a die reports `supposing`.

### 1.4 The overview (R3)

`views::overview` (`views.rs:1361`) lists, before what systems provide:

- the workspace's own cells on nodes and on edges: scan `Graph::written` for every entity, keep names
  `everything()` doesn't own, and count users, with one example value;
- its kinds: targets of `is_a`, each with how many instances and the cells it gives them;
- its tables.

What systems provide becomes one line per component unless `full: true` is asked for.

### 1.5 The MCP tools (R4, R12)

Rewrite `crates/recollect-server/src/mcp.rs` with five tools, and delete the rest:

| Tool | Does | Replaces |
|---|---|---|
| `overview` | the vocabulary, and the instructions that apply to this workspace | `get_overview` |
| `read` | a `ReadSpec` | `get_node`, `list_nodes`, `search_nodes`, `get_tree`, `read_world`, `get_decisions`, `get_work` |
| `apply` | ops, as now | `apply` |
| `changes` | events since a seq, newest first, each with the nodes and edges touched, before and after, in `SavedNode` shape | `get_history` |
| `checks` | what's broken, notices, and what's unresolved (R14, Phase 5) | `find_checks` |

- **Layered instructions.** `INSTRUCTIONS` becomes a short core (ops, edges carrying cells, how to read
  and write) and a section per system, chosen from the workspace's `systems` in `overview`'s answer. The
  sections live beside each system, as `pub const GUIDE: &str` in `notes.rs`, `formulas.rs`,
  `structure.rs` (Phase 2) and `plan.rs`. The modelling guide arrives with Phase 2.
- **Named specifications.** The core instructions show three or four specs to copy: one node, a tree,
  decisions, work.
- **`changes`** needs a seq per event, which `log.rs` has. Before and after are taken from the ops of
  each event.

Tests: `crates/recollect-server/src/tests.rs`, the MCP section rewritten for the five tools. Delete the
tests of deleted tools. The scratch client used for the requirements (`initialize`, then `tools/call`)
is a good smoke test.

**Done when:** every old MCP tool is gone, a read of one node is about the size of its file, and the
formula bug is fixed.

### What Phase 1 did differently

- **Work and decisions are `read {plan: true}`,** not specifications of components. What the plan says about them (where work
  stands, and what a decision is holding up) is worked out by `views::planning` for the worlds in play, not held in cells, so it comes
  as a section of the read rather than per node.
- **`changes` gives before and after as ops:** what each event did, and its inverse, which is what it replaced, each with names
  beside handles. Before and after as whole files would mean replaying the log for each event.
- **The overview gives `last_seq`,** so an agent has a seq to ask `changes` from even before anything has happened.
- **The guides are on each system** (`System::guide`), and the overview gives the guide of each system the workspace uses, lists
  the ones it doesn't, and gives theirs when asked for with `also`.
- **The server's tests weren't mostly MCP,** as this plan said: none of them called MCP. The MCP tests are new, in
  `crates/recollect-server/src/mcp/tests.rs`.
- **YAML keys come out sorted,** since the answer passes through `serde_json::Value`, so `id` isn't first. Worth fixing if it
  reads badly.

## Phase 2: structure without work (R1, R10)

### 2.1 A structure system

New `crates/recollect-systems/src/structure.rs`, listed in `SYSTEMS` before `plan`.

- **Moves from `plan.rs`:** `PART` with `Need` and `Dropped`; `PART_OF`; `DECISION`, `OPTION`,
  `ALTERNATIVE`, `NARROWED`, `UNCERTAIN`, `WITHIN`, `EXCLUDES` and `KNOWING`; `RuledOut`, weights and
  `chosen`; `need`, `in_play_parents` and `selects`; `picks`, `pinned` and `not_ruled_out`; and the checks
  `chosen_must_be_a_live_option`, `decision_without_options`, `one_option_left`,
  `one_decomposition_per_link` and `unknown_cannot_be_chosen`. That last one moves only if `UNKNOWN`
  moves; otherwise it stays in plan.
- **Stays in `plan.rs`:** `Done`, blockers, scope, `CanStart` and `CanFinish`; `WORK`, `TASK`, `GOAL`,
  `QUESTION`, `RISK` and `UNKNOWN`; `DEPENDS_ON`; `work()` and the schedule's use of them. `plan` adds its
  cells to `PART` with `reg.cell(PART, ...)`, which works because `structure` installs first.
- **`dropped` goes with `need`,** because `need` reads it, and "deliberately out of the picture" means
  as much in a design as in a plan.
- **`build`** (`lib.rs:75`) adds `structure` whenever `plan` is wanted, as it refuses `schedule`
  without `plan` today. `needed()` gives `structure` for a workspace that only has decisions and parts,
  so a model never takes on `plan`.
- **`part` isn't written in a model's files.** The app badges components that are `given` or come from
  a `kind` (`badgesOf` in `web/src/theme.ts`). `part_of` already gives `part` to both ends
  (`source_has` in `registry.rs` means "its source has this too"), with `why: edge`, so there's no
  badge unless someone writes `part` as well, as kept's agent did. The fix is for `apply` to drop a
  written component that an edge already gives, or to refuse it with a message saying the edge gives
  it, and for the modelling guide to say so.
- **Imports.** `views.rs`, `ops.rs`, `formulas.rs` and the toy tests import from `structure` where they
  used `plan`.
- **The app.** The Work tab already shows only when something has `work` (`App.tsx:70`). Check the
  Outline's option label: "Needed when …" should read "Under *Option*" for a model.

Tests: `tests/plan.rs` and `tests/plan_cost.rs` still pass unchanged, which is the proof that `plan`
lost nothing. A new `tests/structure.rs` covers a model with decisions and no `plan`: what's under an
option not taken is needed in no world, and no plan cell is present.

Examples: regenerate. Workspaces whose only use of plan is decisions (`trolley`, `monty-hall`,
`which-die` and others) should change from `systems = ["plan"]` to `["structure"]`. Check each diff.

### 2.2 A kind's notes aren't its instances' (R10)

Make `body` per entity in `notes.rs` (`.per_entity()` on its cell), so an instance doesn't take it from
its kind, and search doesn't find it. A kind's description stays its own `body`.
Tests: `crates/recollect-graph/tests/kinds.rs` and a search test in `read/tests.rs`.

### 2.3 The modelling guide

`structure.rs`'s `GUIDE` says how to model a design: kinds for each sort of thing, `part_of`, decisions
and options, and reading with a spec. It's extended in Phases 3 and 4.

**Done when:** a workspace with concepts and decisions has no `plan`, no Part badges and no Work tab,
and dims what's under an option not taken.

## Phase 3: leaning, and looking through a formula in the app (R7)

### 3.1 Leaning

In `structure.rs`:

- a `leaning` component, `.requires(DECISION)` and `.per_entity()`;
- its cell holding `{ref: option}`, typed and checked as `chosen` is;
- an optional `why` text cell, per entity;
- a Notice `leaning_must_be_a_live_option`, modelled on `chosen_must_be_a_live_option`, for a leaning
  towards an option that's ruled out, gone, or not one of the decision's.

Reads show `leaning` with the decision. The app's Decisions tab (`DecisionsView.tsx`) marks the option
leaned to, with its `why`. The Map marks it too, lightly.

Tests: `tests/structure.rs`. `all(d in with.leaning: d.chosen = d.leaning)` used as `where` narrows two
open decisions, one leaned, from four worlds to two.

### 3.2 The worlds chip builds a formula

- `WorldBar.tsx` and `WorldPanel.tsx` show what's pinned and ruled out as the formula it amounts to, and
  take a formula typed in. The text box's formula and the clicks combine with *and*.
- `api.ts`'s `view(world)` becomes `view(world, where)`. `dispatch.rs`'s `WorldArgs` gains `where`, so
  the server and the wasm worker both get it from one change.
- The worlds count stays as it is, and the panel shows `undecided` and `supposing` from 1.3.

Check in the app on an example with two decisions: type the *current intent* formula, and the count and
the Map follow.

**Done when:** a leaning can be set in the app and by `apply`, and the same formula narrows a read and
the app.

## Phase 4: relation kinds, realising, and reading by kind (R6, R8, R9)

### 4.1 Relation kinds as nodes

- **A relation kind is a node** with a `relation` component (in `notes.rs`). Its cells:
  - `arranges` (bool), and `reversed` (bool);
  - `from` and `to`, optional kinds, as `{ref}`;
  - the cells every edge of the kind has, as defaults, written on the relation kind's node as a kind's
    cells are.
- **An edge refers to it.** `relates` stays the one generic edge component. Its `role` cell is replaced
  by `kind: {ref: relation}`. An edge takes the cells it doesn't say from its kind: extend
  `taken_from_kind` and `kinds_of` in `graph.rs` (around lines 1039–1123) so that an edge's kinds are
  the node its `kind` cell refers to. That's a registry setting beside `inherit_through`, such as
  `inherit_by_ref("kind")`.
- **Indexed by kind.** `Graph` keeps a map from relation kind to edges, updated where links and cells
  change. `edges_from` and `edges_into` take a relation kind's name where they take a component. In
  `formulas.rs` (`"in"` and `"out"`, line ~735), `in.names` resolves `names` to a component first, then
  to a relation kind by name. A name that's both is refused as ambiguous.
- **`link {kind: "names"}`** in `ops.rs` makes a `relates` edge with `kind` set.
- **Arranging.** The `View`'s `arranging` list, which the Map and Outline use
  (`OutlineView.tsx:24`), includes relation kinds that say `arranges`. A Notice reports an arranging
  loop.
- **Checks.** A Notice says when an edge's ends aren't the kinds its relation kind says.
- **The app.** The Map labels an edge with its kind's name. The Inspector shows and sets it.

Tests: `crates/recollect-graph/tests/kinds.rs` for an edge taking cells from its relation kind;
`tests/expr.rs` for `count(x in in.names: true)`; `server/src/tests.rs` for `link {kind}`.

### 4.2 Realising and coverage (R8)

- **`realises`** is a built-in edge in `notes.rs`'s `RELATIONS`.
- **The coverage view** is a read specification, `coverage: {kind: Concept, by: layer}`, and a Coverage
  section in the app. It has a row per instance of the kind and a column per `layer` value. The columns
  are in the order the layers are reached along `realises` from the kind's instances, and each cell
  names what realises the row's concept in that layer. A blank is a gap.
- In the app it's a tab that shows only when something `realises` something, as Work shows only for work.

### 4.3 Reading by kind (R9)

- `instances(Kind)` in `formulas.rs`: what links to `Kind` by `is_a`, directly or through kinds of
  kinds.
- A tree read follows any arranging edge or relation kind, with each node's first line of notes.

**Done when:** a model can say "a header names many parents by hash" as an edge of kind *names*, lay
out by it, and list what isn't yet realised in each layer.

## Phase 5: checks, the map, and working together (R14, R11, R13, then R12–R17 as needed)

### 5.1 What's unresolved (R14)

A function in `recollect-core` (`unresolved(o) -> Vec<CheckView>`), worked out when `checks` is called
or the Checks tab opens, never on commit. It lists:

- a cell name used once;
- a label with no target, and an edge to a missing node;
- a cell a kind promises that an instance doesn't have;
- duplicate names where something refers to one by name;
- a relation kind used once.

It shows in the Checks tab (`ChecksView.tsx`) as its own group.

### 5.2 Checks a workspace writes (R11)

A `should` component in `formulas.rs`, `.requires(VALUE)`, like `REQUIRES`, but reported as a Notice
naming the instance when its value isn't true, and never narrowing worlds. Written on a kind, it runs
for each instance through inheritance.

### 5.3 A map you can read (R13)

In `MapView.tsx`: lanes by a chosen cell (`layer`); focus on a node and its neighbours within one or two
edges; collapse below a depth.

### 5.4 Then, as modelling shows which matter

- R15: markdown export of a subtree.
- R16: decision records, and ADR export.
- R17: move, re-kind and merge as single ops in `ops.rs`.

## What Phases 2 to 5 did, and didn't

**Built**

- **Structure** is a system of its own. Its registration and checks are in `structure.rs`, and the code
  that works out `need`, decisions and options is still in `plan.rs`, which `structure.rs` calls. Moving
  that code is only a move, left for later. A cell now records the system that gives it, so writing
  `done` still takes on `plan`. Samples say the systems they use, which a test checks, and the examples
  were written again. Most samples that said `plan` only had decisions, and now say `structure`.
- **Decisions** come back for a design that isn't a plan, so the Decisions tab works there too.
- **A kind's notes** are its own.
- **Leaning** and its `why`, the notice, the Decisions tab and a mark on the Map. The worlds panel takes
  a formula to look through, with one click for current intent.
- **Relations**: a relation is a node, an edge names it in `kind`, takes its cells, and is followed by its
  name. Relations can arrange the Map, Outline and tree reads, with notices for loops and for ends of the
  wrong kinds. `apply` links by a relation's name, and resolves names inside `{ref}`.
- **Realises and coverage**, in reads and as a Coverage tab.
- **What's unresolved**, as a standing list in Checks, and **`should`**, a workspace's own checks.
- **The Map**: lanes by a cell, and only so many levels. Focus was already there.

**Done differently**

- **`instances(Kind)` isn't a function.** `k.in.is_a` does it with one label for the kind, now that a
  path can go along edges anywhere, and `count(self.in.realises)` works on a kind for each instance.
  Instances of instances aren't followed by a formula yet, though a read by kind follows them.
- **A formula typed in the app can't name nodes yet,** since naming them needs labels, so the worlds panel
  keeps pins as pins rather than showing them as a formula.
- **The unresolved list** has the checks that turned out cheap and useful: a cell written once, a letter
  or two from another; nodes sharing a name; `relates` without a kind; a relation used once. It leaves
  out labels with no target and dangling edges, which formulas and loading already report.

**Not done**

- The Inspector doesn't show or set an edge's relation yet; `apply` and the agent do.
- R15 (markdown export), R16 (decision records) and R17 (restructuring ops) are left for when modelling
  shows they're needed, as the plan said.
- The app labels `need` as "Needed", which reads oddly in a design.

## Risks and what to check early

- **`in` mid-path (1.1)** may reach into how paths bind in `formulas.rs`. Refusing first keeps Phase 1
  short.
- **`where` needs a node to be evaluated on (1.3).** The whole may not see `with.leaning` the way a node
  would. Check this in the first test, and if it doesn't, evaluate on a hidden node as `looked_at` does
  with `FOUND`.
- **Moving structure out of plan (2.1)** is the largest diff. Doing it as a pure move first, with no
  behaviour changes and `tests/plan.rs` passing, then the `needed()` and badge changes, keeps it
  reviewable.
- **Edges inheriting from a referenced node (4.1)** is new to `graph.rs`. The memo and depth tests
  (`tests/memo.rs`, `tests/depth.rs`) should be run on every commit of that step.
- **The server's tests** (`server/src/tests.rs`, 1,400 lines) are mostly MCP. Rewriting them is a
  large part of 1.5.
