# Recollect kernel: spec

**Status:** historical. `crates/recollect-kernel` implements this and its tests pass, but the app doesn't use it, and never did.

**Replaced by [graph-core.md](graph-core.md).** Everything here has moved there, and was checked against this implementation on the way. The two differ on purpose in a few places, listed there, including `Done` above a leaf and two bugs in `blockers` here. This crate stays only as what the tests compare against.

**Goals:** modelling flexibility, behaviour that's easy to reason about formally, full auditing of actions, and no layered heuristics.

## Principles

1. **One fact, one place.** Nothing is stored twice, so nothing can disagree with itself.
2. **Stored and derived are separate.** Derived values are pure functions of stored facts and are never saved. You can delete them all and get identical results back.
3. **Behaviour comes from components.** Entities and links are bags of components. A component nothing declares is kept and has no effect.
4. **Nodes are cheap.** Options, gaps and small details are ordinary nodes. Reasoning and viewing are transforms over the graph, so adding nodes doesn't add clutter.
5. **Every value is exact.** "Is this needed?" returns a logical formula over the open decisions, not a score. Probability is an optional layer on top that nothing structural depends on.
6. **Every change is an audited event.** State is the replay of an append-only log.

## Stored facts

```yaml
# Entities and links are bags of components. Unique per (from, to).
calib:         { decision: {} }              # or { decision: { chosen: cad } }
calib.unknown: { unknown: {} }               # a gap: options nobody has named
cad:           { title: Init from CAD }      # an option is an ordinary node
imu:           { task: { estimate: 1 }, done: {} }

links:
  calib.unknown → calib: { alternative: {} }
  cad    → calib: { alternative: {} }        # OR. Also { ruled_out: {...}, weight: 2 }
  board  → calib: { alternative: { ruled_out: {} } }
  imu    → calib: { part_of: {} }            # needed whichever option is taken
  static → cad:   { part_of: {} }            # shared work is simply two parents
  static → free:  { part_of: {} }
  assoc  → static: { depends_on: {} }
```

A link with no components is a plain association. One link can hold several components.

| Component | On | Meaning |
|---|---|---|
| `part_of` | link | Child → parent: AND |
| `alternative` | link | Option → decision: OR. Holds `ruled_out` and `weight`. Its target must be a decision |
| `depends_on` | link | From waits for to. `{on: "decided"}` waits for the decision only |
| `decision` | entity | `{chosen?}`. Holding one value makes "exactly one chosen" unbreakable |
| `done` | entity | Stated complete |
| `dropped` | entity | Deliberately out of the picture. Only ever set on purpose |
| `task`, `goal`, `question`, `risk` | entity | Has work of its own, which only `done` discharges |
| `unknown` | entity | A gap: something here isn't specified yet |

Everything else (`title`, `threatens`, `review`, ...) has no kernel meaning until something declares it.

## Unknown: gaps are nodes

An `unknown` node says what the graph doesn't know yet. It's an obligation nothing can discharge: it can't be done or chosen, and can't have parts. **Removing it is the explicit claim that the gap is closed**, and because removal is a logged event, the claim has an author and a reason. It can be undone.

- **Under a decision, as an option:** the options may not be complete. New decisions get one by default (`new_decision`), and `close_gap` removes it.
- **Under any node, as a part:** there may be more work here (`mark_incomplete`). This covers what the option form can't, because it still counts after a decision is made. A chosen option with a gap isn't done.

A gap has zero weight unless given one, so it doesn't distort probabilities.

Since a gap is a real option, work shared by all the named options isn't `always` needed: `need(shared) = calib∈{cad,free}`. That's honest, but with default gaps nothing under an open decision would ever read as safe, so two levels are reported from the same formula:

- **`holds`:** in every world.
- **`holds_among_known`:** in every world where each decision lands on a named option.

## Worlds

A **world** is one assignment of every open decision to one of its live options. A chosen decision is a constant, and a ruled-out option isn't a value, so ruling out shrinks the set of worlds. Every derived value is the exact set of worlds in which it holds, shown as a formula:

```
always | never | calib=cad | calib∈{cad,free} | root=a ∧ x=p ∨ root=b
```

- **`always`:** true in every future.
- **Not `never`:** possible.
- **The formula itself:** exactly what would make it true, which is also its explanation.

Representation is a reduced, hash-consed multi-valued decision diagram. Equal functions get equal handles, so `f == always` is a constant-time check. Outer decisions are ordered first, so formulas read top-down.

## Kernel cells

**`need(n)`**: the worlds in which `n` is part of what's being attempted.
```
dropped                       → never
no decomposition parents      → always
otherwise OR over parents     part_of(p)     → need(p)
                              alternative(d) → need(d) ∧ d selects n   (never if ruled out)
```

**`all_required_done(n)`** and **`done(n)`**: two columns, as in a spreadsheet.
```
done(n) = stated done ∨ all_required_done(n)      dropped is neither

all_required_done(n) =
    every live part done
  ∧ if a decision: OR over live options, selects(o) ∧ done(o)
  ∧ if it has neither parts nor options: it has no work of its own
```

There's no special case for options. A node with nothing required of it is complete, so "red" is complete, and what keeps "which colour?" open is its gap.

**`blockers(n)`**: what `n` waits for, by name, with the worlds in which it does: its own dependencies and those of everything it's inside. **`blocked(n)`** is the OR of them.
```
depends_on (default)        → need(target) ∧ ¬done(target)
depends_on {on: "decided"}  → target is an open decision
```

**`actionable(t)`**: for a task with no live parts, `need ∧ ¬done ∧ ¬blocked`. Equal to `always` means safe to start now.

**Pressure of an open decision:** `Free` if any work under it is safe among the named options, `Blocking` if only work that depends on it remains, `Idle` if neither, `Decided` if it's not open.

### The semantics of completing a choice

| Situation | `done` |
|---|---|
| No option finished | `never` |
| Option `a` finished, others not | `d=a` |
| All named options finished, gap still open | `d∈{a,b}` |
| All options finished, gap closed, undecided | `always`, and `decision.chosen` is still empty, a separate fact |
| Chosen, and its work is finished | `always` |
| Chosen, but the option has a gap | `never` |

What a dependent waits for is declared on its own link: `depends_on` waits for the work, and `{on: "decided"}` waits for the choice.

## Compute model

A **cell** is a function of one node, as in a spreadsheet. Cells read the store and each other only through `Cx`:

```rust
struct Remaining;
impl Cell for Remaining {
    const NAME: &'static str = "remaining";
    type Value = Cond<f64>;
    fn eval(cx: &Cx, id: &str) -> Cond<f64> {
        let terms = cx.with::<Task>().into_iter().map(|(t, task)| {        // task is a real Task
            let open = cx.and(cx.cell::<Need>(t), cx.not(cx.cell::<Completed>(t)));
            cx.when(open, &cx.pure(task.estimate.unwrap_or(0.0)), &cx.pure(0.0))
        });
        cx.sum(terms.collect::<Vec<_>>())
    }
}
```

The engine provides:

- **Demand-driven evaluation with caching.** There's no declared order.
- **A recorded trace.** Every read is noted, so `d.trace` says what each value came from. Dependencies can't be declared wrongly because they aren't declared. This also makes incremental recompute possible later.
- **Reported cycles.** A cell that depends on itself takes a fallback value at that point and is listed in `d.cycles`. Unrelated cells are unaffected.

### Values

| Type | Meaning |
|---|---|
| `F` | Yes/no, per world: the canonical formula |
| `Cond<T>` | Any value, per world: mutually exclusive guards covering every world, with equal values merged |
| `Option<Id>`, `usize`, ... | Doesn't vary by world |

`Cond<bool>` isn't a `bool`, so `if need(x)` doesn't compile. You write `when`, `map` or `zip`, because the answer differs between worlds. That's what keeps results exact.

### Typed and untyped

Every primitive has both forms, and the typed one is built on the untyped one, so a plugin with no Rust types can do everything a typed cell can.

| Typed | Untyped |
|---|---|
| `with::<T>()` → `(id, T)` | `with_raw("name")` → `(id, json)` |
| `get::<T>(id)`, `has::<T>(id)` | `raw(id, "name")`, `components(id)` |
| `links_into::<L>(id)`, `links_from::<L>(id)` | `links_into_raw(id, "name")`, `links_from_raw(id, "name")`, `links_of(id)` |
| `cell::<C>(id)` | `cell_untyped("name", id)` → `Cond<json>` |
| `registry.add::<C>()` | `registry.add_untyped("name", domain, eval)` |

Enumerating by type guarantees the value is there and well formed.

### Schema

Each component's rules are data: where it goes (entity or link), whether it's an obligation, whether it scopes, what a link's target must have, and how to validate it. A Rust type generates its entry, and a plugin can declare one with no type:

```rust
registry.declare(ComponentDef::entity("review").obligation());
registry.declare(ComponentDef::link("annotates").scoping());
```

Commits refuse malformed values of declared components. A hand edit can still put one on disk, so `reconcile` records and reports it, the store keeps it, and compute treats it as absent. Typed reads never see a bad value.

### A system

A system is whatever installs component rules, cells and checks into a `Registry`. The kernel's own (`kernel()`) are written with this same API.

## Scope

A node's **scope** is derived, never stored: its immediate dominator over the links declared as scoping (`part_of` and `alternative` by default). That's the nearest node every path from the top must pass through.

- One parent: it lives inside that parent.
- Several parents: it lives inside the closest thing they share. `static` under both `cad` and `free` lands in `calib`.
- Parents that share nothing: top level.

Written as two recursive cells (`Scope` and `ScopeDepth`), it needs no maintenance: adding a second parent moves the node. It ignores `dropped` and decisions, so nothing jumps around when you decide.

### Views are transforms

A `ViewGraph` is nodes and edges, and each transform returns another, so they compose:

- **`retain(pred)`:** filter, for example `d.possible("need", id)`.
- **`collapse(d, expanded)`:** fold everything into its outermost unexpanded scope. Links reaching inside are redrawn to what's visible and counted, and links wholly inside a fold disappear with it.
- **`within(d, root)`:** look inside one scope, showing whatever links reach across the boundary, marked external.

What's expanded is the viewer's state and never touches the store.

## Invariants

Refused at commit time, and a refused commit changes nothing and leaves no event:

- `no_self_links`
- `one_decomposition_per_link`
- `chosen_must_be_a_live_option`
- `no_deadlock`: the wait graph is acyclic. A parent waits for its children, anything waits for its dependencies, and a child inherits its ancestors' dependencies.
- `unknown_cannot_be_done`, `unknown_cannot_be_chosen`, `unknown_has_no_parts`
- From the schema: `malformed_component`, `component_on_wrong_target`, `link_target_lacks_component`

**Notices** are precise statements about the graph, not errors: `stated_done_with_open_work`, `decision_without_options`, `dropped_part_of_live_work`.

**What-if** is compute on a hypothetical store: the same code as the real thing, writing and logging nothing.

## Action log

Each event records `seq, author, why, ops, inverse`, and it's the only way state changes.

- **Replay:** `state_at(seq)` reproduces the store at any point, and so any derived value too.
- **Blame:** who last set a component, when and why.
- **Undo:** appends the exact inverse, so history is never rewritten and the undo is itself audited.
- **External edits:** `reconcile` diffs files against the store and records ordinary ops by author. It always records and reports any rule the edit broke rather than refusing it.

## How it's checked

- **34 scenarios** in `tests/scenarios.rs`, and **19 compute and view tests** in `tests/compute.rs`, written from outside the crate using only what a plugin can use.
- **Brute force** in `tests/exactness.rs`, on random graphs with separately written evaluators:
  - `need`, `done` and `blocked` in every world of 400 graphs
  - `scope` against the definition of a dominator, on 300 graphs
  - probabilities against a weighted count of worlds
  - undo and `reconcile` restoring the store exactly
  - `collapse` never losing a node, being idempotent, and changing nothing when everything is expanded
- **Mutation check:** deliberately breaking blocker inheritance and scope's common-parent step each fails its brute-force test. This was run by hand and isn't part of the suite.
- **The demo graph** (`cargo run -p recollect-kernel --example demo -- .demo`): 52 entities and 61 links derived in about 2 ms.

| Scale (release build) | Time |
|---|---|
| 921 entities, 40 independent open decisions (2⁴⁰ worlds) | 9 ms |
| Worst case, 16 pairs of nested decisions under one shared task | 33 ms |
| Worst case, 20 pairs (40 decisions) | 0.86 s |
| A chain 20,000 levels deep, including building it | 1.3 s |

The worst case grows about 5× per extra pair, so by extrapolation that shape hits a wall somewhere around 50 open decisions all sharing one task. Everything is recomputed from scratch on each change.

## What each iteration changed

1. **First version.** Decisions never touch children, so the reopen bug is gone by construction. "Which colour?" could never complete.
2. **Options with no work.** Patched with a special rule: being chosen is their one obligation.
3. **Real data.** `all_required_done` became its own column, blockers are named, pressure has an explicit `Decided`, what-if is a pure function of a hypothetical store.
4. **Harder checks.** Independent brute force, and adversarial scale.
5. **Gaps as nodes.** `unknown` replaced the special rule from iteration 2, which also resolved the case of a finished option next to one with no work, and removed the notice about depending on a plain note.
6. **Compute model.** Hand-ordered systems became demand-driven cells with typed and untyped reads, a schema as data, and values other than yes/no. Fixed along the way:
   - A hand-edited cycle crashed compute with a stack overflow. It's now reported.
   - A misspelled cell silently read as `never`. It now panics, and typed access can't misspell.
   - Undoing a creation left an empty node or link behind instead of removing it.
   - `reconcile` lost a new node that had no components.
   - Deep graphs overflowed the stack. The stack now grows on demand.

## Limits of the compute model

- **Purity is by construction, not proof.** `Cx` is the only access to the store, but Rust can't stop a cell reading a global.
- **Numeric growth.** `Cond<T>` costs one case per distinct value. Sums of many independent estimates can multiply cases, and floating-point sums that differ slightly won't merge. Not measured at scale.
- **Link endpoints are checked at load, not compile time.** The schema guarantees an `alternative` targets a decision, but `links_into::<Alternative>` doesn't carry that in the type.
- **Cycle fallback.** Values computed through a cycle are deterministic but not meaningful. They only arise from a hand edit, and are always reported.
- **Only decisions create worlds.** A cell can't introduce its own uncertainty.

## The current app gets these wrong

- **Wrong probabilities:** paths are assumed independent, giving 62.5% where the truth is 75%. A unit test asserts the wrong value.
- **Stale state:** decisions write `status: dropped` onto children, which then disagree with the decision after reopening.
- **State in prose:** reasons are parsed out of notes, and bring-back searches note text.
- **Scoped edges plus an options struct:** the duplication that prompted this work.

## Not built yet

- The on-disk format for the log and for links
- UI and MCP on this kernel
- Migrating existing workspaces (the demo importer is a one-way sketch that doesn't preserve history)
- The TypeScript runtime. `add_untyped` is the interface it would plug into, and a test stands in for it with a Rust closure that passes only strings and JSON. Guards are opaque handles today, so how TypeScript combines them is undesigned.
- Incremental recompute
