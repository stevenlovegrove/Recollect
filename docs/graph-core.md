# Recollect graph core

**Status:** in use: the app runs on this. The engine is `crates/recollect-graph`, and what's built with it is `crates/recollect-systems`. It replaces everything in [kernel-spec.md](kernel-spec.md), and `recollect-kernel` stays only as what the tests compare against. For the app, see [DESIGN.md](../DESIGN.md).

## Terms

| Term | Meaning |
|---|---|
| **Entity** | A node, identified by a time-ordered UUID handle. The row |
| **Component** | A kind of data, always JSON-serialisable, identified by name. The column |
| **Cell** | The data for one (entity, component) pair. It may be absent |
| **Link** | A directed edge, holding a bag of components. Its *kinds* are those components |
| **Storage** | What backs a cell: a stored value, or a rule |
| **Rule** | A function of one node and what its declared links reach |
| **System** | Whatever installs components and rules |
| **Resource** | Something shared by rules that isn't part of the graph |
| **Op** | One written-down change to the graph |
| **Event** | One commit: who, when, why, its ops, and what undoes them |
| **Check** | A rule saying what's wrong with a node: an invariant refuses, a notice only reports |

## The core

The whole model:

```rust
#[component]
struct Amount(f64);

#[component]
struct Addend;

#[rule("sheet/sum", incoming(Addend))]
struct Sum;

impl Compute for Sum {
    type Output = Amount;
    fn compute(&self, cx: &Cx) -> Result<Amount, Error> {
        let mut total = 0.0;
        for from in cx.sources::<Addend>()? {
            total += cx.value::<Amount>(from)?.0;
        }
        Ok(Amount(total))
    }
}

let mut reg = Registry::new();
reg.rule::<Sum>();

let mut g = Graph::with_registry(Arc::new(reg));
let (price, total) = (g.spawn(), g.spawn());
g.set(price, Amount(10.0));             // a cell backed by a value
g.set(price, Name("Widgets".into()));   // a name is just another component
g.set(total, Sum);                      // a cell backed by a rule, recorded and not yet run
g.link(price, total, Addend);           // the edge the rule follows
g.get::<Amount>(total);                 // Result<Option<Amount>, Error>
```

`set` takes anything that converts into `Storage<C>`, the enum of a value or a rule. `g.set_default` does the same for every node that doesn't say otherwise.

A rule's `Output` is a component, or `Option` of one if it may decide a node has none.

For a throwaway rule, `rule("name").incoming::<Addend>().compute(|cx| ...)` takes a closure instead. It runs the same way, but it can't be saved.

`#[component]` adds the derives and names the component after the type in snake case, or as given by `#[component("name")]`.

Inside a rule:

| Read | Gives |
|---|---|
| `cx.own::<C>()` | This node's component, if it has one |
| `cx.own_value::<C>()` | This node's component, or `Error::Missing` |
| `cx.get::<C>(e)` / `cx.value::<C>(e)` | The same, for a node the rule's links reach |
| `cx.sources::<L>()` / `cx.targets::<L>()` | The nodes at the other end of `L` links |
| `cx.incoming::<L>()` / `cx.outgoing::<L>()` | The same, with what each link holds |
| `cx.with::<C>()` | Every node with `C`, for a declared scan |

A rule only declares what it follows. Edges are separate data, so a rule can apply before any exist, and adding one invalidates what follows it.

### Identity

An `Entity` is an opaque handle, and nothing in the core looks anything up by name. `Name` is an ordinary component: two nodes can share one, and renaming a node reruns only what reads its name. Handles are UUIDv7, so they sort in creation order and nodes created on two machines never collide. The toys refer to nodes by name for readability, through a test-only helper over handles (`tests/support`).

- **Storage is per cell.** The same component can be typed in on one node and computed on another, and a cell can switch between them.
- **`Option` is presence.** A rule can decide a node has no such component. Enumerating by type only yields nodes that have it.
- **`Result` is failure.** A cycle, an undeclared read, a missing required value, a malformed value or a panic in the rule becomes an error in that cell and whatever reads it, like `#REF!`. Other cells are unaffected, and it heals when the cause goes.

### What a rule may read

A rule declares the link kinds it follows, in each direction, plus any component it scans. It may read its own node, the neighbours those links reach, and nodes holding a scanned component. Anything else is `Error::Undeclared`.

So a rule is local: anything further away has to come through a neighbour's own cell. This is enforced at runtime rather than compile time.

### Memoisation

Built in, and it persists across edits. Each computed cell remembers its value, what it actually read, when it was last confirmed correct, and when its value last changed. On a query:

1. Confirmed at this revision: return it.
2. Otherwise check what it read. If none of that changed, it's confirmed without running.
3. Otherwise run the rule. If the value comes out the same, its "last changed" doesn't move, so readers don't rerun.

Nothing runs until queried. The declared links say what a rule *may* read, and the recorded reads say what it *did* read, which is what invalidation uses.

Step 2 checks reads **in the order they were made**, and that matters. A rule is deterministic, so if its first reads are unchanged then so is its next. Checking in any other order can run something the rule no longer reads, part way through evaluating something else, which reports a loop that isn't there.

### Rules as data

A rule is JSON under a name, like a component. Its key says what code it is, its struct fields are its parameters, and the Rust type is the typed shortcut:

```rust
#[rule("puzzle/is", outgoing(About))]
struct Is { choice: Entity, value: String }

g.set(clue, Is { choice, value: "3".into() });
g.set_rule(clue, "holds", "puzzle/is", json!({ "choice": choice, "value": "3" }));   // the same, by key
```

A **system** is a function that adds to a `Registry`, and registration is explicit, so what's active can be read from one place:

```rust
fn install(reg: &mut Registry) {
    reg.rule::<Is>();
    reg.set_default(ChoiceOf);   // on every node that doesn't say otherwise
    reg.runtime_rule(RuntimeRule { key, output, inputs, compute });   // no Rust type
}
```

`registry.list()` gives every rule's key, output, inputs and parameter schema. A rule whose inputs depend on its parameters overrides `Compute::inputs`. `formulas.rs` is one rule whose parameter is a formula's text: see [expressions.md](expressions.md).

A key is normally tied to one component. `reg.rule_on::<C>(key, build)` says a key can also fill `C`, built from its parameters, and it's still refused on anything it hasn't said it can fill. A rule about another rule can set `Inputs::like` to read whatever the rule on that component of its node can, and `cx.own_rule` to read how that rule is written.

| Situation | Behaviour |
|---|---|
| Unknown key, such as a missing plugin | Kept as written, reported on load, and the cell reads `Error::UnknownRule`. It works once something registers it |
| Rule on the wrong component | Kept as written, reported on load, and the cell reads `Error::WrongRule` |
| Bad parameters | Kept as written, reported on load, and the cell reads `Error::Malformed` |
| Link to a node with no file | Kept, reported on load, and the node exists with nothing on it |
| Same rule and parameters set again | No change, so nothing reruns |
| Saving a closure | `SaveError::Closures`, naming the cells, rather than a silent drop |
| Saving a rule the registry can't load | `SaveError::Unregistered`, since the file couldn't be read back |
| Rule's meaning changes | By convention it gets a new key (`core/sum@2`), and the old one stays registered while files use it. Nothing enforces this |

### On disk

`save_dir` and the log both write `nodes/<id>.yaml` and `graph.yaml`:

```yaml
id: 01a19494-2088-7032-86b4-3a0d85b43f54
components:
  name: Total
  supplier:
    days: 12
rules:
  amount:
    rule: sheet/sum
links:
- to: 01a19494-2088-7033-a1f2-6b8c11d09e42
  components:
    addend: null
```

Values and rules sit in separate sections, so plain data stays plain and a value can't be mistaken for a rule. A link lives with its source. Memos are never saved. `graph.yaml` holds only defaults that differ from the registry's, so a default in code takes effect when the code changes. Files whose text is unchanged aren't rewritten, so a commit shows only real changes. Files for nodes that are gone are removed, and anything that isn't named like a node file is left alone.

### Concurrency

A `Graph` is `Send + Sync`, as are rules and every component, including world values. Reading takes `&self` and writing takes `&mut self`, so shared use is an `RwLock<Graph>`: many readers, or one writer. The memo sits behind its own lock, so concurrent readers can fill it safely and a cell still runs once.

Today, readers take turns on that lock while rules run, so an expensive rule delays other readers. That's internal: finer-grained evaluation can replace it without changing the API, but it would need to detect cycles that span threads, which isn't built.

### Typed and runtime components

JSON under a name is what a component really is. `Runtime { name, value }` is that form directly, and a Rust type is a shortcut for looking one up:

| Typed shortcut | Runtime |
|---|---|
| `set(e, Amount(1.0))` | `set_runtime(e, "amount", json!(1.0))` |
| `get::<Amount>(e)` | `get_runtime(e, "amount")` |
| `set(e, rule)` | `set_runtime(e, "amount", rule)`, with a `Rule<Value>` |
| `link(a, b, Addend)` | `link_runtime(a, b, Runtime::new("addend", json!(null)))` |
| `cx.incoming::<Addend>()` | `cx.incoming_runtime("addend")` |
| `cx.with::<Amount>()` | `cx.scan("amount")` |

The two are interchangeable on one cell: a value stored by name reads back typed, and a rule with no Rust type can be read by a typed rule, and vice versa. A typed cell keeps its native value in the memo as a cache, so typed reads don't pay for JSON. A runtime value of the wrong shape is `Error::Malformed` in that cell when read typed, and the data itself is untouched. Runtime rules get the same declared-link checks and memoisation. This is the interface a TypeScript system would use.

## The log

A graph held by a `Log` only changes through `commit`: `log.graph()` gives `&Graph`, so the types enforce it. Its state is the replay of its events.

```rust
let (mut log, problems) = Log::open(dir, registry, "whoever edited the files")?;

let seq = log.commit("steven", "CAD model is already accurate", vec![
    Op::set(calib, Decision { chosen: Some(cad) }),
])?;

log.blame(Target::Node(calib), "decision");   // who last set it, when and why
log.history(Target::Node(calib));             // everything that touched it
log.state_at(seq);                            // the graph just after that event, as a new Graph
log.undo(seq, "steven", "changed my mind")?;  // appends the inverse; history isn't rewritten
log.checkout(seq, "steven", "try another way")?; // go to any event; the next commit follows from it
log.what_if(&ops);                            // the same code as a commit, writing nothing
log.reconcile(&files, "git pull")?;           // an outside edit, as ordinary audited ops
```

There are five ops: `Spawn`, `Despawn`, `Set` (a value or a rule by key, or `None`), `Link` (one kind, or `None`) and `Default`. They hold only written-down forms, so a closure can't get into a log. `Op::set` takes the same `.into()` values as `Graph::set`, and panics on a closure.

A commit is all or nothing. It applies its ops, runs the checks, and if anything is newly broken, takes the ops back out and returns `CommitError::Refused` with what and where. Nothing is logged and no file changes.

### History is a tree

Every event has a `parent`, and the log has a **head**: where it is now, and what the files show. `checkout` moves the head to any event, so the next commit starts a branch from there if that isn't the end of one. Nothing is rewritten and nothing is lost. Events are numbered in the order they happened, whichever branch they're on, and the head moving is a record of its own in the log, so who moved it and when is known and it survives a reopen.

`blame`, `history` and `path` look only at what led to the head. `undo` refuses an event that isn't on the way here, because its inverse wouldn't mean anything from here. `state_at` follows an event's own ancestors.

Going somewhere takes the shortest way: back to where the two branches split using inverses, and on from there using ops. So memos survive, and only what differs reruns. A random test checks this lands exactly where replaying from nothing does, every cell compared, after every one of 400 moves and commits.

A crash while going back leaves it either there or where it was, with the files caught up and no false "edit". An edit on disk that hasn't been noticed yet should be recorded before moving, so it stays on the branch it was made on, which the app does. Logs from before there were branches read as one straight line.

### Invariants and notices

A check is an ordinary rule in its own column, on every node, returning what's wrong there:

```rust
reg.invariant(rule("unknown_cannot_be_done"), |cx| {
    let both = cx.own::<Unknown>()?.is_some() && cx.own::<Done>()?.is_some();
    Ok(if both { vec!["a gap can't be done: remove it to close it".into()] } else { vec![] })
});
reg.notice(rule("decision_without_options").incoming::<Alternative>(), |cx| ...);
```

So checks are memoised, local and declared like any rule, and a plugin's are the same as the system's. Setting `Done` on a node that isn't a gap reruns nothing, because the check above stops reading before it reaches `Done`.

What's about a value alone is checked only on the ops' targets: `reg.on_nodes::<Task>()` and `reg.on_links::<PartOf>()` declare where a component goes and its shape. A commit is also refused if it names a rule that can't run. A link from a node to itself is always refused.

Rules already broken, such as by a hand edit, don't block other work. Only what's newly broken is refused, and `log.violations()` lists what stands.

`plan.rs` holds the planning system's invariants, described under [Planning](#planning). **`no_deadlock` needed no graph search:** every node has two cells, `CanStart` (needs its parents started and what it depends on finished) and `CanFinish` (needs it started and its parts finished). Work that can never complete is exactly a cycle among those, which the core already reports.

### On disk, and crashes

```
log.jsonl      what's true: append-only, one record per line
log.applied    how far the files have caught up
nodes/*.yaml   a projection of the log, and the place to edit by hand
graph.yaml
log.lock       so two processes can't write one log
```

A commit appends a line to `log.jsonl` and flushes it, **which is the moment it happens**. Then it replaces the affected files, each atomically, flushes, and moves `log.applied` on. Opening repairs whatever a crash left:

| What it finds | What it does |
|---|---|
| A last line with no newline | That commit never returned. It's cut off and reported as `TornLog` |
| A file equal to the state at `log.applied`, or to the latest state | It's at most behind, so it's rewritten silently |
| Any other difference | Someone edited it. It's recorded as an event under the `author` given to `open`, and it stands, even if a crash happened too |
| A complete but unreadable line | `OpenError::Corrupt`, naming the line, since a crash can't cause this |

If the log can't be written, the commit is undone and returns `CommitError::Io`. If only the files can't, it did happen, so `CommitError::FilesBehind` says so and the next commit repairs them.

`Graph::save_dir` is still there for a graph with no log. Each file is replaced atomically, but a crash can leave some newer than others.

### Auditing derived values

Replaying a log restores stored facts and rule keys, but derived values come from the code running now. Two things cover that:

- **Which code:** the log records the host's build id (`reg.build(..)`) and every rule's key, inputs and parameter schema, whenever they change. `log.code_at(seq)` reads it back, and `open` reports `CodeChanged`. Rule bodies can't be recorded this way.
- **What it meant then:** `log.witness(node, component)` records what a cell reads as now. `log.audit()` replays the log under today's code and returns a `Drift` for every witnessed cell that reads differently. The stored facts are identical by construction, so that can only be the code. This catches a bug fix under the same key, with the same signature and the same build id.

The audit only covers what was witnessed. The intent is for the app to witness what it shows the LLM or a person when a decision rests on it, which isn't built.

### Differences from the old kernel

Deliberate, and the only ones the comparison below allows:

- **A link is its kinds.** Removing the last kind removes the link, so a plain association needs a kind of its own. The kernel keeps empty links.
- **Undoing a link's creation** removes the kind that event added, not the kinds added later. The kernel removes the whole link. The comparison only undoes the latest event, where the two agree.
- **Unit components hold `null`,** not `{}`.
- **A deadlock is reported at every node that can never finish,** not once. So after a hand edit leaves a loop, adding a new wait on it is refused, where the kernel would allow it.
- **A malformed value also stops checks that read it,** which report that they couldn't check. The commit is refused either way.

## Planning

`recollect-systems/src/plan.rs` is a system like any other: components and rules written with the API a plugin uses, so the core knows nothing about it. `plan::install(&mut reg)` adds it.

### Stored facts

| Component | On | Meaning |
|---|---|---|
| `PartOf` | link | Child → parent: AND |
| `Alternative { ruled_out, weight }` | link | Option → decision: OR. Its target must be a decision |
| `DependsOn { on }` | link | From waits for to. `on: decided` waits for the answer, not the work |
| `Decision { chosen }` | node | An open question. Holding one value makes "exactly one chosen" unbreakable |
| `Done` | node | Stated complete. Only counts at a leaf |
| `Dropped` | node | Deliberately out of the picture |
| `Task`, `Goal`, `Question`, `Risk` | node | Work of its own, which only `Done` discharges |
| `Estimate` | node | A task's days, typed in or a formula, so it can differ between worlds |
| `Unknown` | node | A gap. It can't be done or chosen, so removing it is the claim that it's closed |

An option is an ordinary node, and nothing is ever written onto a decision's children, so deciding and reopening can't leave stale state behind.

### Derived, per world

A **world** is one way every open decision could land. Each of these is a rule, memoised like any other, and its value is the exact set of worlds in which it holds. `Bool::show` gives that as a formula, such as `calib∈{cad,free}`.

| Cell | Value |
|---|---|
| `Choice` | How a decision could land. A chosen one is a constant, and only live options that aren't ruled out are worlds |
| `Need` | `never` if dropped, `always` with no parents, otherwise any parent it's in play through |
| `Completion` | `Some(true)` finished, `Some(false)` owed, `None` not in the completion system. See below |
| `Blockers` | What it waits for, by name, with the worlds in which it does: its own dependencies and those of the parents it's in play through |
| `Blocked` | Any blocker |
| `Actionable` | For a live task with no live parts: needed, owed and not blocked |
| `Scope`, `ScopeDepth` | The nearest node every path from the top passes through. Decisions and `Dropped` don't move it |

`plan::pressure` says whether an open decision is holding work up, and `plan::explain` gives one level of why a node reads as it does. `ViewGraph` in `view.rs` gives filtered, collapsed and scoped views, as pure transforms.

### Completion comes from the leaves

Completion is something a node opts into. A leaf is in the system if it holds an obligation, a `Done` or an `Unknown`, and everything above takes the combination of what's beneath it: all its live parts, and a decision's selected option. A decision that lands on nothing is owed. A node with nothing completable beneath it reads `None`, and waiting on it costs nothing.

| Situation | `Completion::open()` | `Completion::finished()` |
|---|---|---|
| A plain note, or "red" with no work | `never` | `never` |
| Which colour, red or blue, with a gap | `colour=colour.unknown` | `never` |
| Option `a` finished, `b` not | `d=b` | `d=a` |
| All options finished and the gap closed, undecided | `never` | `always`, and `chosen` is still empty, a separate fact |
| Chosen, but the option has a gap | `always` | `never` |

**`Done` above a leaf changes nothing**, so there's one source of truth, and `stated_done_with_open_work` reports any disagreement. To finish something early, drop what's left: that's a fact with an author, and `dropped_part_of_live_work` reports it.

### Scheduling

`schedule.rs` works out dates over the same facts, in every world. A task's estimate is its duration, and it starts once what it depends on has finished and whatever it's in play through has started. `Finish` is 0 where a node isn't needed, so waiting on it costs nothing, and `None` where a gap makes it unknowable. `Slack` comes from following the same links backward. `Dates` says the same for drawing: when it happens, in the worlds that need it.

### What a plugin can add

`plan::install_with` takes a `Config`, with the components that are obligations and the kinds of link that scope. Beyond that, a plugin's rules read these cells like any others. `Remaining` in `tests/plan.rs` is a remaining-effort total per world, and `a_rule_with_no_rust_types_can_read_and_combine_these` does the same kind of thing with only JSON crossing the line.

### Differences from the old kernel's rules

Each of these is handled explicitly by the comparison below:

- **`Done` above a leaf** overrode what's beneath it there. There's also no separate `all_required_done` column, since nothing stated overrides it any more.
- **Blocking is only inherited through parents in play.** There, shared work waits on a phase this world isn't doing.
- **Two routes to the same blocker hold in either.** There, the last one reached replaces the others, so something explicitly waiting for an answer could read as free.
- **A malformed value is an error in the cells that read it,** rather than being treated as absent.
- **`Completion` says `None`** where the kernel's `done` said `always` for a node with nothing to do. What depends on it behaves the same.
- **Variables are ordered by creation, not sorted by depth.** A decision's `Choice` reads its own `Need` first, so whatever it's inside gets its variable first and formulas read from the outside in. A decision added above an existing one comes after it until the process restarts. The value is the same either way, only how it prints differs.

## Worlds, on top

`graph.rs` has no knowledge of worlds. They're values in `worlds.rs` that a rule can return:

- **`Space`:** the variables and the shared diagram. Values normally live in one global space, behind a lock.
- **`Var`:** one open choice.
- **`Bool`:** the exact set of worlds where something holds. Supports `&`, `|`, `!`, `iff`, `implies`, `prob`, `count`, `solutions` and `show`.
- **`Cond<T>`:** any value per world. Supports `map`, `zip`, `when`, `holds`, and for numbers `expected`, `min` and `max`.
- **`Worldly<T>`:** a `Cond<T>` that's written plainly when it's the same in every world, for a cell that may be typed in or worked out.

A variable is identified by its name, options and weights, so formula handles stay valid across edits. Equal functions get equal handles, so early cut-off works on them like any other value.

Variables and options are usually entity handles as strings, so `show_with` translates them for reading.

Any choices can be pinned: `Space::picks` turns a world into `Picks`, and `Bool::given` and `Cond::given` narrow a value to the worlds consistent with them, with the picked choices gone from it. That's exact, and costs one walk of the diagram. It's how the app answers a what-if, and why it doesn't have to enumerate anything.

World values are JSON too. A `Bool` is a table of its diagram, which stays linear in size where listing worlds wouldn't, and is rebuilt by logic on the way in, so it means the same in a space with a different variable order. A `Cond<T>` is a list of `{when, value}`. So a rule in another language can receive one and return one, with no shared memory.

## The toys

These were written to find out whether the core is general enough, and they became systems and sample workspaces. `tests/toy_*.rs` in `recollect-systems` hold their tests, and `examples/` the workspaces.

### 1. Spreadsheet (formulas)
Invoice, Fibonacci, compound interest, as formulas. Also: editing one number reruns only what it reaches, an unchanged result stops there, circular references are errors that heal, formulas and typed values swap, whole-column rules, scans, and an untyped rule.

A trip budget then shows the same sheet with its choices held open as decisions: min, max, expected, and `Over £200 = Stay=Hotel`.

### 2. Schedule (`schedule.rs`)
`tests/toy_gantt.rs` has classic critical-path scheduling with no worlds, written in the test and sharing nothing with the system: forward over `depends_on` for dates, backward over the same links for slack, summary tasks rolling up from leaves. It's what checks the system: fix a decision, and the plain schedule with no idea worlds exist must agree, including where a task's estimate is a formula over a decision. Slack is checked from its definition, by slipping a task by exactly that much and seeing that the end doesn't move, and by one more and seeing that it does.

### 3. Puzzles (formulas)
A choice is a node with a `Domain`, a constraint is a formula such as `a != b`, and the solutions are the worlds where a `requires` node's `all(constraint)` is true. Map colouring, n-queens, 4×4 sudoku, knights and knaves, and the zebra puzzle.

| Puzzle | Result | Time (release) |
|---|---|---|
| Australia, 3 colours | 18 solutions | Not timed |
| 8 queens | 92 solutions | 8 ms |
| 4×4 sudoku | 288 grids | 5 ms |
| Zebra | 1 solution: the Japanese owns the zebra | 31 ms |

Counts match published values, and the colouring and sudoku also match plain search written in the tests.

## How it's checked

| Check | Evidence |
|---|---|
| Memoisation is invisible | 23,165 random edits, with several landing between reads, compared with a new graph, including 122 cycle answers |
| Saving is invisible | Every toy saved to real files, reloaded, and compared cell by cell, plus 13,812 cells across the random edits. Dropping rule parameters or a link on the way makes these fail |
| The log and invariants agree with the old kernel | 7,200 random steps through both: 2,673 accepted, 4,527 refused, 544 undos. Same verdict, same rules broken and same resulting graph every time, with all 11 refusal rules exercised. A malformed value only has to match on that rule |
| Refusals and undos leave no trace in memos | 187,612 cells, planning cells included, compared with a new graph replaying the same events |
| Reconciling is exact | 200 pairs of random states: it lands exactly on the target, as one event, and the inverse lands exactly back |
| Going anywhere in the tree is exact | After each of 400 random moves and commits, every cell compared with replaying from nothing. A real process killed at each step of going back |
| A crash leaves all of a commit or none of it | A real process killed at each of the 19 steps of a commit, and 21 more times during the repair. Each time: the right state, no false "edit", files caught up, and the log still working. Also a torn line at many lengths, and a hand edit made while the files were behind |
| The checks can fail | 17 deliberate breakages of the core and log (rollback removed, files before log, marker frozen, deadlock rules cut, audit disabled, and so on), and 31 of the planning rules (each kernel bug put back, dropped or ruled-out work counted, and so on), all caught. Five survived at first. Four showed gaps in the tests, now closed, and one showed a redundant check, now deleted |
| Planning agrees with the old kernel, in every world | 268,905 node-world comparisons across 800 random plans with edits (6,008 accepted, 2,616 refused), through both logs: need, probabilities, completion, blocking, actionable, scope, pressure, notices and views. Each is compared on the differences above, and blocking is compared in one direction only where a node has several parents. 794 states, where the kernel's second bug can occur, skip the comparison of blocking with the kernel |
| Planning agrees with plain evaluators | The same worlds, against need, completion and blocking written out in the test, sharing no code with either. This is what vouches for the blocking the kernel gets wrong |
| Planning scenarios | 51, mostly the kernel's own, through the real write path |
| A real restart | A separate process solves the zebra puzzle from its files, starting with an empty diagram |
| Worlds agree with plain scheduling | Every node in every world of 300 random plans, against a schedule that knows nothing of worlds, and every leaf's slack checked by actually slipping it |
| The app, from outside | 32 API tests on working copies of the samples: every sample opens with no error in any cell, narrowing, commits, refusals, undo, proposals, hand edits, reading an old state, and that the originals never change |
| Formulas mean what they say | Random typed formulas in every world against a plain evaluator with no world values in it, sharing only the parser. Narrowing against really deciding, through a commit. 32 deliberate breakages, 30 of them run and caught, and 2 that remove a cap and would hang left unrun. See [expressions.md](expressions.md) |
| Formulas are enough | Three problems nobody wrote rules for, solved with data alone through the same path Claude uses: a budget cap on a plan, a clause that isn't about a pair, and a tiered price and a loan with a macro |
| The samples mean what they should | Each committed workspace opened as the app opens it, with its known answers checked |
| Memoisation saves work | 9,835 rule runs where new graphs needed 38,299 |
| Puzzles | Known counts and independent search |

## Bugs the toys found

- **Stale memo:** a formula swapped for a typed value and back reused its old memo, so readers kept the typed value. Fixed: a memo only stands for an unbroken run of being computed.
- **False cycles:** memos checked their old reads in sorted order, so a batch of edits could run a rule the reader no longer used, report a loop that wasn't there, and memoise it. In the log this hid a real deadlock: the false one was already "standing", so the real one didn't count as new. The comparison with the old kernel found it. The earlier random test had missed it because it read after every single edit. Fixed by keeping reads in order, with a direct test.
- **Stale memos after swapping a default:** the same bug as the first, for a whole column. Found while writing this, and fixed.
- **Lock held by a spawning process:** a lock belongs to the open file, so a subprocess being spawned elsewhere could briefly hold one for a log that had just closed, and opening it failed with `Locked`. Stress runs found it. Fixed by retrying for a moment.
- **Inheriting from parents not in play:** a shared task inherited dates and blocking from a parent that this world isn't doing. Fixed here by inheriting only through `in_play_parents`. **`recollect-kernel` has the same bug in `blockers`**, and its brute-force check repeats the same definition, so it can't catch it.
- **A blocker replaced by another route:** `recollect-kernel` keeps only the last route to each blocker, so a task waiting for an open decision's answer could read as free if it also inherited an ordinary dependency on that decision. The comparison found this, and the plain evaluator settled which side was right. The kernel is left as it is, since nothing uses it but the tests.
- **A view looping forever:** following scopes after a hand edit made a loop never ended, in the first version of the port. A scenario found it. The kernel had guarded against this.

## Limits

- **Loops spanning worlds:** which cells read which doesn't vary by world, so a loop no single world contains is still reported as a cycle (`a_loop_that_no_single_world_contains_is_still_reported`). This is pinned by a test, not solved.
- **Rule locality is enforced at runtime, not compile time.** Declaring a link type doesn't stop `cx.incoming::<Other>()` compiling.
- **Variable order is creation order.** The puzzles work partly because clues are added before all-different constraints and choices are created in a sensible order. Worst cases remain exponential.
- **The diagram is never garbage collected,** and the global one is shared by every graph in the process. Long-lived processes with many edits to option sets will grow.
- **Numeric growth:** `Cond<T>` costs one case per distinct value, not yet measured at scale.
- **Reads take turns.** Concurrency is safe but not yet parallel, as described under Concurrency. World values also share one lock on the global `Space`.
- **Rules can't capture state.** What a closure would capture becomes parameters, so a predicate becomes data, such as a formula's text. That's readable and saveable.
- **A commit's cost grows with the graph.** Each one checks every check's memo at every node, even though little reruns. See the table below.
- **Importing many nodes is slow on disk,** because each file is flushed separately. It's rare, but it's what a workspace migration would do.
- **A process dying is tested, but losing power isn't.** Flushes are in place for it, but nothing here can show the disk honoured them.
- **The log only grows,** and opening replays all of it. There are no snapshots.
- **Branches don't merge.** You can go between them, but nothing brings two together.
- **One writer.** Two machines appending to one log through git would conflict, and nothing merges them yet. Files merged by git and then opened are fine: they're recorded as an edit.
- **Rule keys' versions are convention.** Nothing enforces `core/sum@2`. The audit notices when a meaning changes, but only for witnessed cells.
- **Reading a whole column is reading each cell.** A rule scanning its own component is a cycle, so totals live in a different component.

## Cost

Costs went up a little when formulas arrived: an edit on 10,000 tasks went from 79 ms to 125 ms, and the zebra puzzle from 16 ms to 31 ms. The estimate became a second op on every task, and every node has three more default cells.

Release build, on one Mac with full flushes, for a plan with phases of tasks and dependencies between phases:

| Tasks | One edit, in memory | One edit, on disk | Import, in memory | Import, on disk | Reopen, 201 events |
|---|---|---|---|---|---|
| 1,000 | 7 ms | 29 ms | 28 ms | 4.6 s | 43 ms |
| 10,000 | 125 ms | 150 ms | 1.4 s | 46 s | 1.1 s |

A plan 20,000 levels deep is checked in 2.7 s without running out of stack.

Reading the plan afterwards, as a screen would (`tests/plan_cost.rs`), with a decision in every tenth phase:

| Tasks | First read of everything | Each edit: commit, then reread everything | Deciding: commit, then reread everything |
|---|---|---|---|
| 1,000 | 10 ms | 6 ms, then 5 ms | 7 ms, then 6 ms |
| 10,000 | 106 ms | 111 ms, then 72 ms | 104 ms, then 71 ms |

Like a commit, a reread costs a check per cell even though little reruns. Reading only what's on screen costs only that.

| Worlds | Time to read |
|---|---|
| 921 nodes, 40 independent open decisions (2⁴⁰ worlds), every cell | 8 ms |
| Worst case, one task under 16 pairs of nested decisions | 34 ms |
| Worst case, 20 pairs (40 decisions) | 1.0 s |

The worst case is the same as the kernel's: about 5× per extra pair.

## Not built

- Computed link components. Links only hold stored ones.
- Witnessing what the app shows. The mechanism is here, but nothing calls it.
- The TypeScript runtime. World values now have a JSON form it can receive and return, but there's no TypeScript library for building or combining them.
- Merging logs across machines.
