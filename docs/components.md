# Components, kinds and edges

**Status:** proposal. Nothing here is built yet. It changes the model in [graph-core.md](graph-core.md), and once it's built that document is rewritten to match.

## Why

The engine grew up with a component meaning one cell, and with a rule as what fills a cell. Since then cells have become formulas, and kinds pass cells down. What's left of the old view shows:

- **Every node has every cell a system defaults.** The plan system works out needed, done, waiting, waits for, can start and scope on every node. In the Vane watch, a case file with no work in it, all 20 nodes carry them, and they say "never", "nothing" and "nothing to finish".
- **The inspector is a wall of values.** Cells are grouped by where they came from ("What it is", "Worked out"), not by what they're about. The badges on a card (Decision, Uncertain) are easy to find, but nothing ties a badge to the cells it means.
- **Links hold data nothing can see.** An option's weight is JSON on its `alternative` link. It can't be a formula, has no default to fall back on, and until recently showed nowhere but the link's own panel.
- **"Rule" means three things.** A Rust function filling one cell; the formula engine, which is one such rule (`expr@1`); and a check. Every computed cell saved in every sample's log is `expr@1`. Choosing some other rule for a cell is plumbing nothing uses.

## The model

### Entities: nodes and edges

An **entity** is a node or an edge. Both have components, cells, formulas and history in the same way. They differ only in that an edge has exactly one source and one target, fixed when it's made, and is drawn as an arrow.

There's one edge per ordered pair of nodes. The several relationships between two nodes, say `alternative` and `label`, are components on that one edge, as they're kinds in one bag today. An edge's id is derived from its pair, so ops can keep addressing it as `(from, to)`. Moving an end means removing the edge and making another. Deleting a node deletes its edges.

Edges join nodes, not other edges, and aren't instances of kinds. Neither would change the model if it were allowed later.

### Components

A **component** is a facet of an entity: Decision, Task, Requires, `alternative`. It's what a badge shows. It declares:

- its **name**, **icon** and a sentence saying what it means;
- whether it goes **on nodes or on edges**;
- its **cells**, each with a name, a type, a sentence, and a **default**: a value or a formula. Some are flagged as worth a line on a card, and some are internal (worked out for other cells, never shown);
- the components it **requires**: on the same entity, or, for an edge component, on its source or target.

Every cell name belongs to exactly one component. `done` is Work's, `weight` is `alternative`'s. A workspace's own cells, such as a car's price, belong to the kind that says them (see below).

**Requiring brings.** Giving a node Uncertain gives it Decision too, if it hasn't one. An edge with `alternative` brings Option to its source and Decision to its target. A brought component shows as its own card, marked as brought by what needs it. It can't be removed while something still needs it, and it goes when the last of them does, unless it was also given directly.

Formulas in a component's defaults can refer to the cells of anything it requires. That's what requiring is for, and the check of a formula on a component's default only has those names in scope.

**Systems can add cells to another system's component.** The schedule system adds start, finish and slack to Work rather than inventing a component of its own. A component's cells are the ones every installed system gives it.

### Kinds

A **kind** is a node that others are an instance of, by `is_a`, or a table whose rows they are. A kind is a component made in the workspace rather than in code:

- An instance has the kind's components. A "Bug" kind that has Task makes every bug a task.
- An instance takes the kind's cells. "Car" with a price formula gives every car a price.
- An instance shows the kind as a card of its own, holding the cells the kind gives.

Components that hold per-instance state can't come from a kind. Decision's `chosen` is one: two instances can't share which option was taken. A component says whether it can.

### Where a cell's value comes from

For a cell on an entity:

1. what the entity says itself: a value or a formula;
2. what its kinds say, nearest first, as now;
3. a workspace override of the component's default, if there is one (today's `Default` op);
4. the default of the component the cell belongs to.

That's only if the entity has the component, directly, from a kind, or brought by another. Otherwise the cell isn't there at all. There are no defaults that apply to every node. So a suspect in the Vane watch has its name, its notes and its own `arrives`, and nothing else.

### Values and formulas, not rules

A cell holds a value or a formula. Nothing else.

What the built-in rules do becomes **functions formulas can call**, written in Rust as now. Components use them in their defaults. For example, Part's `need` defaults to `need()` and Decision's `choice` to `choice()`. The functions see what rules see today: the entity, its edges, and the cells they reach, with every read tracked.

This removes:

- the `Compute` rule types and `#[rule]`, which become built-in functions;
- `SavedRule`, `Stored::Rule`, `Storage::Rule`, `set_rule` ops, `RuleInfo` in the API, and `Cell.source: "rule"`;
- `defaults_to` and `fills`, which are just a component's cell defaults now;
- `RuntimeRule`, a rule with no Rust type meant for plugins, which becomes a function a plugin gives formulas.

**Checks** keep their own name and their own registration. An invariant refuses an edit, and a notice reports. They're about whole entities, not cells, so they stay a separate thing.

### Formulas on edges

A formula on an edge reads its ends as `from.` and `to.`, as a node's reads `self.`. `alternative`'s weight defaults to `if(has(from.unknown), 0, 1)`, which is the special case in `ChoiceRule` today.

A formula that goes over edges keeps binding the node at the other end. `o in in.alternative` still gives the options. `o.via` is the edge it came through, so `o.via.weight` is that option's weight. Existing formulas don't change.

## Components, as they'd be defined

What each system's components become, roughly. The cell names are today's where there's one.

**Base** (on every node, from no system): `name`, `body`.

**Notes**

| Component | On | Cells | Requires |
|---|---|---|---|
| Sources | node | `sources` | |
| Drawing | node | `drawing` | |
| serves, supports, contradicts, constrains, supersedes, relates | edge | none | |

**Formulas**

| Component | On | Cells | Requires |
|---|---|---|---|
| Value | node | `value` (typed, a formula, or a choice from its options); internal `value_type` | |
| Options | node | `options` (today's `domain`). Brings a choice: `value` defaults to choosing one of them | Value |
| At | node | `row`, `col` | Options |
| Requires | node | none: its condition is `value`, which has to be true | Value |
| Uncertain | node | none of its own: it says the choice isn't anyone's to make, so what's expected averages over it | Decision |
| Macro | node | `params`, `body`; internal `macro_body` | |
| label | edge | `called`: the name the source's formulas use for the target | |
| is_a | edge | none | |

**Plan**

| Component | On | Cells | Requires |
|---|---|---|---|
| Part | node | `need` = `need()`; internal `can_start`, `can_finish` | |
| Work | node | `done`, `dropped`, `waiting`, `waits_for`, `can_start`, `scope`; internal `scope_depth` | Part |
| Task | node | `estimate` | Work |
| Goal, Question, Risk | node | none of their own | Work |
| Unknown | node | none of its own: a gap, still owed until it's closed | Work |
| Decision | node | `chosen` (stored, per instance), `choice` = `choice()` | Part |
| Option | node | `belief` = `belief()`, `prior_belief` = `prior_belief()`, given only when its decision is uncertain | Part |
| part_of | edge | none | Part on its source |
| alternative | edge | `weight`, `ruled_out` | Option on its source, Decision on its target |
| depends_on | edge | `wait_for` (done, or started) | Work on its source |

**Schedule** adds to Work: `start`, `finish`, `slack`, and the quiet `dates`; internal `latest_start`, `latest_finish`. `plan_end` is a cell on the whole graph, which is an entity with a component of its own.

A few things follow from these tables:

- **Belief and prior belief become cells.** The server stops working them out on the side, and `web/src/summary.ts` stops special-casing them. They're Option's cells, flagged for cards, like any other.
- **Requires stops showing a second "Value" badge.** Value is brought by Requires, so its card sits under Requires'.
- **Needed is on what's part of something,** brought by `part_of` and `alternative` edges, and on work and decisions. A plain note on its own, a suspect with nothing beneath it, or a piece of evidence has none. Needed still flows down through a plain note that's part of something, because that note has Part.
- **Where a weight lives can still be decided separately.** It's on `alternative` above, as today, but now as a proper cell. Moving it to Option is a one-line change to these tables.

## What the app shows

**The inspector is a stack of component cards**, one per component the entity has, kinds included, each with its icon:

- **Collapsing.** A card can be collapsed. Which cards are collapsed is remembered per component, not per node.
- **Its cells.** A card lists its component's cells, each showing what it comes to and where that came from: the component's default, a kind, or typed here. Any cell can be typed over, and clearing it goes back to where it came from.
- **What it brings.** A brought component shows under what brought it.
- **The rest.** Name and notes stay at the top. A node's own cells that no kind gives go in a card of their own. The "What it is" and "Worked out" sections go.

**A card on the map** shows the components as badges, as now, and the cells their components flag for cards. Clicking a badge opens the inspector with that card in view.

**An edge's inspector** is the same component cards. **A node's link lists** show each edge's flagged cells beside it, as they show weights now, from either end.

The **Table view's columns** are cells, as now, but only those of components something in view has.

**The API** sends each entity's components with their cells, and the components' definitions (name, icon, about, on, cells, requires) in place of today's `ComponentInfo` and `RuleInfo`. Edges get ids.

## The log

Ops become:

- `spawn` and `despawn`, as now;
- `add` and `remove` for giving or taking away a component;
- `set` for one cell: a value, a formula, or nothing;
- `link` and `unlink` for an edge with a component;
- `default` for a workspace override of a component's cell default.

Old logs are upgraded as they're read, and never rewritten:

- **A marker component** set on a node, such as `task` or `uncertain`, becomes `add`.
- **A component that held a struct** becomes `add` plus `set`s of its cells. Decision `{chosen}` becomes `add decision` and `set chosen`.
- **`Stored::Rule { rule: "expr@1", params: { text } }`** becomes a formula. No other rule appears in any saved log. Anything else is reported, not guessed at.
- **A link with a value** becomes `link` plus `set`s of the edge's cells. `{weight: 3}` becomes `set weight 3` on the edge.
- **The samples are regenerated,** and `samples::FORMAT` goes up.

## The kernel crate

`recollect-kernel` is the first prototype ([kernel-spec.md](kernel-spec.md)). It's only a dev-dependency now, kept as a second implementation that `log_vs_kernel` and `plan_vs_kernel` check the engine against. Its model is the old one, a component as one value and links as bags, so after this it can't agree with the engine on anything interesting.

It's retired, along with those two tests. In their place:

- **The plan's semantics are tested explicitly,** including the mixed cases this makes matter: a plain note holding tasks, an option holding a note, a goal with a plain part, and a decision under a plain note.
- **The random-edit test keeps everything that needs no second implementation:** replaying a log agrees with the live graph, and with its files reopened, and refusals are the same both ways.

## Building it

Each stage leaves the app working, with every test passing.

1. **Edges as entities.** Give edges ids and cells, and turn link values into edge cells. Formulas get `from.`, `to.` and `.via`. The app's link lists read edge cells.
2. **Components with cells.** Component definitions in the registry: cells, defaults, requirements, bringing, and which can come from a kind. A node's components are stored explicitly, and lookup is the four steps above. The systems declare their components, and universal defaults go.
3. **Rules into formula functions.** The built-in rules become functions, and the rule plumbing goes from the engine, the ops, the API, the MCP instructions and the docs.
4. **The log.** The new ops, the reader that upgrades old logs, the samples regenerated, and the format bumped.
5. **The app.** Component cards, badges tied to them, edge inspectors, and belief and prior belief as ordinary cells.
6. **The kernel retired,** with the tests above in its place, and graph-core.md rewritten.

Stages 1 and 2 can go either way round. Stage 4's log changes would be easiest done with stage 2, since that's when the shape of what's stored changes.

## Still to decide

- **Where an option's weight lives:** on the `alternative` edge, as above, or on Option.
- **What a brought component's card is called** when what brought it is an edge. For example, Option is brought by an `alternative` edge to "Who took it?".
- **Whether `name` and `body` are a component** that every node has, so that a kind could give a default name, or stay outside the model.
- **Whether a kind can say a component is not there,** so that an instance takes most of a kind's components but not one of them. Nothing needs it yet.
