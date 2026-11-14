# Components, kinds and edges

**Status:** in use: the app runs on this. The engine is `crates/recollect-graph`, and the systems built on it are `crates/recollect-systems`. For the app, see [DESIGN.md](../DESIGN.md), and for the formula language, [expressions.md](expressions.md).

## Terms

| Term | Meaning |
|---|---|
| **Entity** | A node or an edge. Both have components and cells in the same way |
| **Edge** | From one node to another, one per ordered pair. It's there while it has a component |
| **Component** | A facet of an entity, such as `decision`, `task` or `alternative`. It's what a badge shows, and it brings cells |
| **Cell** | One named value on an entity. It's only there if the entity has the cell's component, or if no component brings it |
| **Default** | What a cell is when nothing says otherwise: a value, a formula, or something built in |
| **Kind** | A node others take components and cells from: what they link to by `is_a`, or the table they're a row of |
| **System** | A function that adds components, their cells and their checks to a `Registry` |
| **Check** | What's wrong with an entity that has some component. An invariant refuses a commit, and a notice only reports |
| **Op** | One written-down change to the graph |
| **Event** | One commit: who, when, why, its ops, and what undoes them |
| **World** | One way every open choice could land |

## The model

### Entities

A node's handle is a UUIDv7, so nodes sort in creation order and nodes made on two machines never collide. An edge's handle is worked out from its two ends, so ops address it as `(from, to)`. Moving an end means removing one edge and making another. Edges join nodes, not other edges.

Several relationships between two nodes, say `depends_on` and `label`, are components on the one edge between them. Taking an edge's last component takes the edge, and everything written on it. A node can only be removed once its edges are gone. The server's `delete_node` takes them first.

Nothing in the engine is looked up by name. `name` is a cell, two nodes can share one, and renaming a node reruns only what reads its name. The tests refer to nodes by name for readability, through a helper over handles (`tests/support`).

The graph as a whole is an entity too, `Entity::whole()`. It has the components that go on the whole graph, such as `schedule`, and nothing can be written on it, so its cells are worked out once however many nodes read them.

### Components

A component declares:

- its **name**, an **icon**, and a sentence saying what it means;
- where it goes: **on nodes, on edges, or on the whole graph**;
- its **cells**, each with a type, a sentence and a default. Some are flagged as worth a line on a card, some as internal (plumbing for other cells), and some as quiet (for a view to read, not a person);
- the components it **requires** on the same entity, and for an edge, the components it brings to its **source** and **target**;
- what it **fills** in: how a cell of a component it requires is worked out on an entity with it, as `decision` says how `choice` lands;
- whether it's **per entity**, so it can't come from a kind;
- whether **every node** has it;
- for an edge, whether it **arranges** a map (and which way up), or says what comes **first**;
- its **checks**, and optionally a **constraint** on which worlds count.

Every cell name belongs to exactly one component. `done` is `part`'s and `weight` is `alternative`'s. A system can add cells to another system's component: the schedule adds `start`, `finish` and `slack` to `part`. A workspace's own cells, such as a car's `price`, belong to no component. Any node can have one, and a formula on one has its type checked as it runs.

**Requiring brings.** Giving a node `uncertain` gives it `decision` too, and `decision` brings `part`. An edge with `alternative` brings `option` to its source and `decision` to its target. A node keeps a brought component while something brings it. Taking a component away only takes away what was given, so one that's also brought stays. The server's `remove` also clears what the node wrote in that component's cells, so nothing is left without its component, and undoing it brings both back.

An entity says why it has each component: it was given it, every node has it, it takes it from a kind, another component requires it, or an edge brings it.

### Where a cell's value comes from

For a cell on an entity that has its component:

1. what the entity says itself: a value or a formula;
2. what its kinds say, nearest first;
3. what a component it has fills in, if one does (`Registry::fill`);
4. its component's default.

If the entity doesn't have the component, the cell isn't there at all. There are no defaults that apply to every node, so a suspect in the Vane watch has its name, its notes and its own `arrives`, and nothing else.

A cell holds a value or a formula, nothing else. A default can also be **built in**: Rust, given with `CellBuilder::builtin` or `Registry::fill`. That's for what can't be a formula, such as what makes worlds (`chosen`), and for what's simpler in Rust (`need`, `blockers`, `scope`). A built-in default has no written form, so it's never saved. A cell can be written over if it holds formulas, or isn't worked out by default.

**Filling in keeps a cell in one place.** `chosen` is `choice`'s cell, whatever's choosing: a decision fills it in over its live options and their weights, and `options` over a plain list. So a formula reads `x.chosen` the same way for both, and a component that's required can stay ignorant of what requires it. A component can only fill a cell of one it requires, which is checked when the graph is made, and two that fill the same cell shouldn't go together.

Taking away what an entity says brings back what it takes from its kind or default. Taking away a component keeps what the entity wrote in its cells, but a commit that leaves a written cell without its component is refused (`cell_without_component`).

### Formulas on edges

A formula on an edge reads its ends as `from.` and `to.`, as a node's reads `self.`. `alternative`'s `weight` defaults to `if(has(from.unknown), 0, 1)`: a gap has no weight unless someone gives it one.

A fold over edges binds the node at the other end, and `o.via` is the edge it came along. So `o in in.alternative` gives the options, and `o.via.weight` is that option's weight.

### Nodes nobody wrote

Some nodes exist only to be read, so they're never saved and nothing can be written on them. A numbered table's rows past its last written one are such nodes: see [expressions.md](expressions.md#tables). So are **constants**, which are only what their cells say: `Graph::constant` gives the node for some cells, the same one for the same cells, and has the components those cells belong to. A plain list of options makes one for each option, with that `name` and `value`, so `a.chosen = b.chosen` is the worlds where two choices land on the same option.

### Kinds

A kind is a node others are an instance of: what they link to by `is_a`, or the table they're a row of (see [Tables](expressions.md#tables)).

- **An instance has its kind's components:** those the kind was given. A "Bug" kind that has `task` makes every bug a task.
- **An instance takes its kind's cells** where it doesn't say them itself. "Car" with a price formula gives every car a price, and the formula runs for each car, with `self` meaning that car.
- **Some things are about one entity alone,** so they're never taken from a kind. A component can be per entity: `decision` is, since two instances can't share which option was taken. So can a cell: `name` and `value` are, so a kind's own answer isn't each instance's, and a table's own `value`, such as a puzzle's `distinct(rows)`, isn't its rows'.
- **A kind can have a kind.** Lookup goes first to the other parts of the same row, then to the kinds of all of them, then those kinds' kinds. Something more specific always comes first. Otherwise it's nearest first, and then the order nodes were made. A loop of kinds goes round once.
- **An instance has all of its kind's components.** Something that's mostly like a kind but lacks one is a different kind, perhaps with what's shared as a kind of its own above both.

Which components and cells a node has depends on its kinds, so memos follow changes to a kind: changing a kind's cell reaches everything that takes it, however far down, and linking or unlinking a kind reaches every cell the node could take from it.

### Checks

```rust
reg.check(UNKNOWN, "unknown_cannot_be_done", Severity::Invariant, |cx| {
    let done = cx.own_is_stored(Done::NAME) && cx.own::<Done>()?.is_some_and(|d| !d.finished().is_never());
    Ok(if done { vec!["a gap can't be done: remove it to close it".to_string()] } else { vec![] })
});
```

A check belongs to a component and only runs on entities that have it. It's worked out in a cell of its own, `check/<name>`, so it's memoised like any other: setting `done` on something that isn't a gap runs nothing here.

A commit also checks what it touched against the definitions: a component nothing provides, a component on the wrong kind of entity, a written cell without its component, a value of the wrong shape, a formula that can't be read, and an edge from a node to itself. These look only at the ops' targets.

`reg.advice` is for what's worth saying about the graph as a whole, which no single entity's cells can see, such as an open decision holding work up. A component's **constraint** says which worlds count on each entity that has it: that's how `evidence` works. `requires` is judged for each way the choices could go, given everything the constraints leave, so it isn't one: see `worlds_left` in `formulas.rs`.

## The components

**Notes** (always there)

| Component | On | Cells | Requires |
|---|---|---|---|
| `named` | every node | `name` (per entity) | |
| `notes` | node | `body`, in markdown | |
| `sources` | node | `sources` | |
| `drawing` | node | `drawing`: see [drawings.md](drawings.md) | |
| `serves`, `supports`, `contradicts`, `constrains`, `supersedes`, `relates` | edge | none | |

**Formulas** (always there)

| Component | On | Cells | Requires |
|---|---|---|---|
| `value` | node | `value` (per entity): typed in, or a formula; internal `value_type` | |
| `choice` | node | `chosen` (per entity): the node it lands on in each world, read through as `x.chosen.cell`. Writing one in settles it. Filled in by what requires it | |
| `options` | node | `options`: a plain list, each a constant with that name and value. Fills in `chosen`, landing on each equally | `choice` |
| `at` | node | `at`: where a choice goes on a board | `options` |
| `requires` | node | `at_least`: how surely it has to be met, which is always unless it says. Demanded of your choices: only the ways they could go that make `value` true that surely count | `value` |
| `evidence` | node | none. Known to be true: only the worlds, and the draws, where `value` is true count. It never restricts a choice of yours | `value` |
| `macro` | node | `macro` (`params`, `body`); internal `macro_body` | |
| `label` | edge | `called`: the name a formula on the source uses for the target. Arranges a map upside down | |
| `is_a` | edge | none. What kind of thing the source is | |

**Plan**

| Component | On | Cells | Requires |
|---|---|---|---|
| `part` | node | `need`, `done`, `dropped`; internal `blockers`, `scope`, `scope_depth`, `can_start`, `can_finish` | |
| `work` | node | `blocked`, shown as Waiting | `part` |
| `task` | node | `estimate`, `actionable` | `work` |
| `goal`, `question`, `risk`, `unknown` | node | none of their own | `work` |
| `decision` | node, per entity | none of its own. Fills in `chosen`, over its live options as likely as their weights say | `part`, `choice` |
| `uncertain` | node | none: it isn't anyone's choice, so what's expected averages over it | `decision` |
| `option` | node | none of its own | `part` |
| `part_of` | edge | none. Brings `part` to both ends | |
| `alternative` | edge | `weight` (default `if(has(from.unknown), 0, 1)`), `ruled_out`. Brings `option` to its source and `decision` to its target | |
| `depends_on` | edge | `wait_for`: `done` (the default) or `decided`. Brings `part` to both ends | |

**Schedule** adds to `part`: `start`, `finish`, `slack`, and the quiet `dates`; internal `latest_start` and `latest_finish`. The whole graph has `schedule`, with `plan_end`.

So `need` is only on what's in a plan's structure: what's part of something or has parts, an option, or work. A plain note on its own, a suspect with nothing beneath it, or a piece of evidence has none. A note that's part of something has `part`, so need still flows down through it.

A belief isn't a cell. It depends on what's pinned, so it's worked out for each view: an option's `chance` is how strongly it's believed among the worlds left, and its `weight` is its share of its decision's weights, before any evidence. The app shows both on the `option` and `uncertain` cards.

## What the app shows

**The inspector is a stack of component cards,** one per component the entity has, kinds included, each with its icon and why the entity has it. A card can be folded away, and which are folded is remembered per component, not per node. A card lists its component's cells, each saying what it comes to and where that came from: written here, a kind, or the default. Any cell that can be written can be typed over, and clearing it goes back to where it came from. The name is at the top. The workspace's own cells go in a card of their own, and those taken from a kind in a card for that kind. An edge has the same cards.

**A card on the map** shows as badges the components a node was given or takes from its kind, apart from `notes` and `sources`, and a line for each cell its components flag as worth one. A badge in the inspector scrolls to its card.

**The API** sends each entity's components, with why it has each, and its cells, and sends the components' definitions in the overview.

## Reading

A computation sees one entity, and anything it can reach from there:

| Read | Gives |
|---|---|
| `cx.own::<C>()` / `cx.own_value::<C>()` | This entity's cell, if it has it, or `Error::Missing` |
| `cx.get::<C>(e)` / `cx.value::<C>(e)` / `cx.get_value(e, name)` | The same, for another entity, typed or by name |
| `cx.incoming(c)` / `cx.outgoing(c)` | The edges with component `c`, each with the node at the other end |
| `cx.sources(c)` / `cx.targets(c)` | Just those nodes |
| `cx.scan(c)` | Every node with component `c` |
| `cx.has(e, c)` | Whether an entity has a component |
| `cx.backed(cell)` / `cx.own_is_stored(cell)` | How one of this entity's cells is backed, and whether a value is written there |
| `cx.ends()` | An edge's source and target |
| `cx.rows()`, `cx.row_along(n)`, `cx.row_at(table, key)` | Tables: see [expressions.md](expressions.md#tables) |

Nothing is declared in advance. Every read is recorded, and that's what invalidation runs on.

A cell's type in Rust is `#[cell]`, and `#[component]` names a component for asking whether something has it. JSON under a name is what a cell really holds, so `get::<C>` and `get_value(e, "name")` read the same cell. A typed cell keeps its native value in the memo, so typed reads don't pay for JSON. A value of the wrong shape is `Error::Malformed` in that cell when read typed, and the data itself is untouched.

`Option` is presence: a computation can decide an entity has nothing in a cell. `Result` is failure: a cycle, a missing value, a malformed value, a formula that can't be read or a panic is an error in that cell and whatever reads it, like `#REF!`. Other cells are unaffected, and it heals when the cause goes.

### Memoisation

Built in, and it persists across edits. Each computed cell remembers its value, what it read, when it was last confirmed correct, and when its value last changed. On a read:

1. Confirmed at this revision: return it.
2. Otherwise check what it read. If none of that changed, it's confirmed without running.
3. Otherwise run it. If the value comes out the same, "last changed" doesn't move, so readers don't rerun.

Nothing runs until it's read.

Step 2 checks reads **in the order they were made**. A computation is deterministic, so if its first reads are unchanged then so is its next. Checked in any other order, it can run something the cell no longer reads, part way through evaluating something else, and report a loop that isn't there.

**A memo only stands for an unbroken run of being computed.** If a cell held a written value, or nothing, in between, the same answer coming back is still a change to its readers.

### Concurrency

A `Graph` is `Send + Sync`. Reading takes `&self` and writing `&mut self`, so shared use is an `RwLock<Graph>`. The memo sits behind its own lock, so concurrent readers fill it safely and a cell still runs once. Readers take turns on that lock while cells are worked out, so an expensive one delays other readers. That's internal: finer-grained evaluation could replace it without changing the API, but it would need to detect cycles that span threads.

## The log

A graph held by a `Log` only changes through `commit`: `log.graph()` gives `&Graph`, so the types enforce it. Its state is where it was opened from, plus the replay of its events. The log lives in memory for as long as a workspace is open, and is what undo, redo and "who did this" work from. It isn't saved: what's saved is the graph, as files.

```rust
let (mut log, problems) = Log::from_saved(saved, registry);   // from files; `Log::new` for an empty one

let seq = log.commit("steven", "CAD model is already accurate", vec![plan::choose(calib, cad)])?;

log.blame(On::Node { id: calib }, "chosen");   // who last wrote it, when and why
log.history(Target::Node(calib));             // everything that touched it
log.state_at(seq);                            // the graph just after that event, as a new Graph
log.undo_last("steven")?;                     // takes back the newest change, by appending its inverse
log.redo("steven")?;                          // takes back that undo, the same way
log.undo(seq, "steven", "changed my mind")?;  // takes back any one event
log.what_if(&ops);                            // the same code as a commit, writing nothing
log.reconcile(&files, "git pull");            // an outside edit, as ordinary audited ops
log.take_changes();                           // the node files to write since this was last asked
```

There are seven ops, each with an exact inverse:

| Op | JSON |
|---|---|
| Make a node | `{"op": "spawn", "id"}` |
| Remove a node, once its edges are gone | `{"op": "despawn", "id"}` |
| Give a node a component | `{"op": "add", "id", "component"}` |
| Take one away | `{"op": "remove", "id", "component"}` |
| Give an edge a component, making the edge if it's new | `{"op": "link", "from", "to", "component"}` |
| Take one off an edge | `{"op": "unlink", "from", "to", "component"}` |
| Write in a cell, or with neither field take away what's written | `{"op": "set", "id", "cell", "value"}`, or `"formula"`, with `"from"` and `"to"` in place of `"id"` for an edge |

The op language the app and Claude write comes down to these. It adds names for handles, `new:x` for a node that doesn't exist yet, `add_node`, `delete_node` (which unlinks the node's edges first), `table`, and `labels` with a formula. Writing in a cell of a component a node doesn't have gives it that component. See `crates/recollect-core/src/ops.rs`.

A commit is all or nothing. It applies its ops, runs the checks, and if anything is newly broken, takes the ops back out and returns `CommitError::Refused` with what and where. Nothing is logged. Checks already broken, such as by a hand edit, don't block other work, and `log.violations()` lists what stands.

`no_deadlock` needs no graph search. Everything in a plan has two internal cells, `can_start` (its parents started and what it depends on finished) and `can_finish` (it started and its parts finished). Work that can never complete is exactly a cycle among those, which the engine already reports.

### Undo and redo

History is a line. Undoing appends the inverse of the newest change not yet undone, and redoing appends the inverse of that undo, so both are ordinary changes: checked like any other, with an author, and refused if they'd newly break something, as when what an undo would bring back has since been ruled out. Each says which event it takes back (`reverts`). Any other change, a hand edit included, ends what can be redone.

### Files

A workspace is files at paths relative to its root:

```
recollect.toml       its name, which systems it uses, and whether Claude may change it directly
nodes/<id>.yaml      a node each, with its edges
proposals/<id>.json  changes waiting for review
```

`recollect-core` reads them and says which to write after each change, and the host, the server or a browser, does the reading and writing. A node's file looks like this:

```yaml
id: 01a1bff4-e98a-7511-803e-a718fc43b670
components:
- notes
cells:
  body: The neighbour. He collects watches, and offered to buy this one in May. She said no.
  from_garden: true
  name: Mr Pryce
formulas:
  arrives: trip.at_house_by
links:
- to: 01a1bff4-e98a-7511-803e-a71642dd9766      # Who took it?
  components:
  - alternative
  cells:
    weight: 3.0
- to: 01a1bff4-e98a-7511-803e-a721a0af9766      # How did Mr Pryce get home?
  components:
  - label
  cells:
    called: trip
```

`components` are the ones it was given, `cells` its values and `formulas` its formulas, so a value can't be mistaken for a formula. Each edge lives with its source, with its own `components`, `cells` and `formulas`. Only what was written is saved: never memos, defaults, what a kind gives or what's brought. Files whose text is unchanged aren't rewritten, so a change shows only what really changed. Files for nodes that are gone are removed.

Any `.yaml` directly in `nodes/` is read, whatever it's called: the node is the `id` inside. A node lives in the file it came from, so one made by hand or by another program as `nodes/an-idea.yaml` stays there and is written back there. Only a new node gets a file named for it.

Nothing in a file needs a system to be read. A component or cell nothing provides goes back out exactly as it came in, and is reported as standing. An edge to a node with no file is kept, reported, and the node exists with nothing on it, and gets a file. A formula that can't be read is kept as written and reported.

When files change underneath an open workspace, by hand, `git pull` or another program, the host says which, and the difference is recorded as an event by `disk`, so it can be undone like anything else. A file that can't be read, such as one caught half written, is left as it was understood before. Files from older versions, such as a `log.jsonl`, are ignored.

Each file is replaced whole when it's written, so a reader never sees half of one, but there's no log on disk any more, so a process stopped part way through a change that touches several files can leave some newer than others. A workspace kept in a browser is written in one transaction, so it can't.

### Long chains on the web

Natively the stack grows as a chain of cells needs it (`stacker`). In WebAssembly it can't, so a chain is worked out in stretches: a cell too deep to finish where it was asked for is put off, worked out from the top, the stretches above it worked out again, and the read tried again. What read a put-off cell is marked as needing to be worked out again, so nothing it gave while it was put off is kept. The answer is the same either way, which `tests/depth.rs` checks against every sample with a limit of 5. A loop too long to see from one stretch ends, after enough tries, in an error rather than going on forever.

## Planning

`plan.rs` is a system like any other, written with the API a plugin uses, so the engine knows nothing about it. `plan::install` adds it, and the formulas system with it.

### What's written

| What | Meaning |
|---|---|
| `part_of` | Child to parent: all of a node's parts are needed |
| `alternative` | Option to decision: only the option taken is needed. `weight` says how far it's believed before any evidence, and `ruled_out` says why it's out |
| `depends_on` | The source waits for the target. `wait_for: decided` waits for the answer, not the work |
| `chosen` | Written in, the option a decision took, as `{"ref": handle}`. Holding one node makes "exactly one chosen" unbreakable |
| `task`, `goal`, `question`, `risk` | Work of its own, which `done` discharges |
| `estimate` | A task's days, typed in or a formula, so it can differ between worlds |
| `unknown` | A gap. It can't be done or chosen, so removing it is the claim that it's closed |

An option is an ordinary node, and nothing is ever written onto a decision's children, so deciding and reopening can't leave stale state behind.

### Worked out, per world

Each of these is the exact set of worlds in which it holds, or a value per world. `Bool::show` gives one as a formula, such as `calib∈{cad,free}`.

| Cell | Value |
|---|---|
| `chosen` | Unless it's written in, how a decision could land: only live options that aren't ruled out are worlds, and with none it lands on nothing |
| `need` | Never if dropped, always with no parents, otherwise any parent it's in play through |
| `done` | Finished, owed, or undefined for nothing to finish. A default formula, so a node can type over it. See below |
| `dropped` | Deliberately out of the picture. Defaults to `false` |
| `blockers` | What it waits for, with the worlds in which it does: its own dependencies and those of the parents it's in play through |
| `blocked` | Any blocker |
| `actionable` | For a live task with no live parts: needed, owed and not blocked |
| `scope`, `scope_depth` | The nearest node every path from the top passes through. Decisions and dropping don't move it |

Blocking is only inherited through parents in play, so shared work doesn't wait on a phase this world isn't doing. Two routes to the same blocker hold in either.

`plan::pressure` says whether an open decision is holding work up, and `plan::explain` gives one level of why a node reads as it does. `ViewGraph` in `view.rs` gives filtered, collapsed and scoped views, as pure transforms.

Variables are ordered by creation. A decision's `chosen` reads its own `need` first, so whatever it's inside gets its variable first and formulas read from the outside in. A decision added above an existing one comes after it until the process restarts. The value is the same either way, and only how it prints differs.

### Done comes from what's beneath, unless a node says otherwise

Being finishable is something a node opts into. A leaf is in the system if it's work, and everything above takes the combination of what's beneath it: all its live parts, and a decision's selected option. A decision that lands on nothing is owed. A node with nothing completable beneath it reads undefined, and waiting on it costs nothing.

| Situation | `Done::open()` | `Done::finished()` |
|---|---|---|
| A plain note, or "red" with no work | `never` | `never` |
| Which colour, red or blue, with a gap | `colour=colour.unknown` | `never` |
| Option `a` finished, `b` not | `d=b` | `d=a` |
| All options finished and the gap closed, undecided | `never` | `always`, and nothing is written in `chosen`, a separate fact |
| Chosen, but the option has a gap | `always` | `never` |

**What a node says for itself replaces this.** That's a value or a formula in the same cell, and taking it away brings the default back, so there's still one place to look. `stated_done_with_open_work` reports a statement that disagrees with what's beneath it.

### Scheduling

`schedule.rs` works out dates over the same facts, in every world, as default formulas with nothing about them that another cell's formula couldn't do. A node can type over any of them, and a date typed in moves whatever follows it. `whole.plan_end` is worked out once, however many nodes read it. A task's estimate is its duration, and it starts once what it depends on has finished and whatever it's in play through has started. `finish` is 0 where a node isn't needed, so waiting on it costs nothing, and undefined where a gap makes it unknowable. `slack` comes from following the same edges backward. `dates` says the same for drawing: when it happens, in the worlds that need it.

### What a plugin can add

A component that requires `work` is an obligation, and the plan treats it as one (`a_plugin_can_add_an_obligation`). Beyond that, a plugin's cells read these like any others. `Remaining` in `tests/plan.rs` is a remaining-effort total per world, and `a_cell_with_no_rust_type_can_read_and_combine_these` does the same with only JSON crossing the line.

## Worlds, on top

`graph.rs` knows nothing of worlds. They're values in `worlds.rs` that a cell can hold:

- **`Space`:** the variables and the shared diagram. Values normally live in one global space, behind a lock.
- **`Var`:** one open choice.
- **`Bool`:** the exact set of worlds where something holds. Supports `&`, `|`, `!`, `iff`, `implies`, `prob`, `count`, `solutions` and `show`.
- **`Cond<T>`:** any value per world. Supports `map`, `zip`, `when`, `holds`, and for numbers `expected`, `min` and `max`.
- **`Worldly<T>`:** a `Cond<T>` that's written plainly when it's the same in every world, for a cell that may be typed in or worked out.

`chance.rs` is the same for what's uncertain and isn't a short list: a `Rand` is a number, or a truth, as an expression over draws, and a `Reader` says what one comes to, exactly where it has a law and otherwise from draws. See [distributions.md](distributions.md).

A variable is identified by its name, options and weights, so handles stay valid across edits. Equal functions get equal handles, so early cut-off works on them like any other value. Variables and options are usually entity handles as strings, so `show_with` translates them for reading.

Any choices can be pinned: `Space::picks` turns a world into `Picks`, and `Bool::given` and `Cond::given` narrow a value to the worlds consistent with them, with the picked choices gone from it. That's exact, and costs one walk of the diagram. It's how the app answers a what-if, and why it doesn't have to enumerate anything.

World values are JSON too. A `Bool` is a table of its diagram, which stays linear in size where listing worlds wouldn't, and is rebuilt by logic on the way in, so it means the same in a space with a different variable order. A `Cond<T>` is a list of `{when, value}`. So code in another language can receive one and return one, with no shared memory.

## The toys

These were written to find out whether the engine is general enough, and they became systems and sample workspaces. `tests/toy_*.rs` in `recollect-systems` hold their tests, and `examples/` the workspaces.

1. **Spreadsheet.** Invoice, Fibonacci and compound interest as formulas, and a trip budget with its choices held open as decisions: min, max, expected, and over £200 exactly when `Stay=Hotel`.
2. **Schedule.** `tests/toy_gantt.rs` has classic critical-path scheduling with no worlds, written in the test and sharing nothing with the system: forward over `depends_on` for dates, backward for slack, summary tasks rolling up from leaves. Fix a decision, and this plain schedule must agree, including where an estimate is a formula over a decision. Slack is checked from its definition, by slipping a task by exactly that much and seeing the end doesn't move, and by one more and seeing it does.
3. **Puzzles.** A choice is a node with `options`, a constraint is a formula, and the solutions are the worlds where every `requires` node is true: with nothing a matter of chance, that's all requiring is. Map colouring, eight queens, 4×4 sudoku, knights and knaves, and the zebra puzzle, which find 18 colourings, 92 solutions, 288 grids, and the Japanese owning the zebra.

## How it's checked

| Check | Where |
|---|---|
| Memoisation is invisible: after random edits, a long-lived graph answers as a new one holding the same state | `recollect-graph/tests/memo.rs` |
| Kinds and tables: the same, through random kinds, renames and edits | `tests/kinds.rs`, `tests/tables.rs` |
| Saving keeps everything, and what's unknown goes back out as it came | `tests/saving.rs` |
| The log: refusals, undo, the tree of history, reconciling and hand edits | `tests/log.rs` |
| A crash leaves all of a commit or none of it: a real process killed at each step of a commit and of the repair | `tests/crash.rs` |
| Concurrent readers agree with one thread, and a cell runs once | `tests/concurrency.rs` |
| Planning scenarios through the real write path | `recollect-systems/tests/plan.rs` |
| Worlds agree with plain scheduling, in every world of random plans | `tests/toy_gantt.rs` |
| Formulas mean what they say | `tests/expr.rs`: see [expressions.md](expressions.md#how-its-checked) |
| The samples mean what they should | `tests/samples.rs` |
| The app, from outside, including held-out problems solved with data alone | `recollect-server/src/tests.rs` |

What things cost is measured by `tests/plan_cost.rs` and `tests/schedule_cost.rs`, which don't run by default: `cargo test --release -p recollect-systems --test plan_cost -- --ignored --nocapture`.

## Limits

- **Loops spanning worlds:** which cells read which doesn't vary by world, so a loop no single world contains is still reported as a cycle.
- **Variable order is creation order.** The puzzles work partly because clues come before all-different constraints and choices are made in a sensible order. Worst cases remain exponential.
- **The diagram is never garbage collected,** and the global one is shared by every graph in the process.
- **Reads take turns,** as described under Concurrency. World values also share one lock on the global `Space`.
- **A commit's cost grows with the graph.** Each one confirms every check's memo on every entity with its component, even though little reruns.
- **Importing many nodes is slow on disk,** because each file is flushed separately.
- **A process dying is tested, but losing power isn't.** Flushes are in place for it, but nothing here can show the disk honoured them.
- **The log only grows,** and opening replays all of it. There are no snapshots.
- **Branches don't merge,** and nor do two machines' logs. Files merged by git and then opened are fine: they're recorded as an edit.

## Not built

- A TypeScript runtime for systems. World values have a JSON form one could receive and return, but there's no library for building or combining them.
- Plugins. Systems are compiled in.
- Merging logs across machines.
