# Expressions

**Status:** built. The language is `crates/recollect-systems/src/expr.rs`, the system is `formulas.rs`, and the tests are `tests/expr.rs`, the held-out problems in the server's HTTP tests, and the rest of the toys. How formulas fit the engine is in [components.md](components.md).

## Why

Without formulas, everything a workspace computes is compiled Rust. To say "last year's balance times one plus the rate", someone has to write Rust, and so:

- Claude can arrange what exists, but it can't state a new relationship. That's the main limit on having an LLM in the loop.
- The library grows by one piece of Rust for each example, each with one user.
- A plan's facts can't be worked out: a task's estimate is a number someone typed, and can't depend on a decision.

## The design in one example

```yaml
id: 01a1...
components:
- value
cells:
  name: Year 3
formulas:
  value: prev * (1 + rate)
links:
- to: 01a1...            # Year 2
  components:
  - label
  cells:
    called: prev
- to: 01a1...            # Rate
  components:
  - label
  cells:
    called: rate
```

A cell holds a formula as its text. A `label` edge names what it reads.

## The decisions

### 1. A formula means its result in each world, separately

> For any world `w`, a formula's value in `w` is what you'd get by evaluating it on ordinary typed values, using its inputs' values in `w`.

The evaluator runs on world values, where `Cond::zip` and `Bool` make that exact and cheap. The meaning doesn't mention worlds at all. So:

- **The lens stays honest.** Narrowing a value and really deciding can't disagree, which is what [DESIGN.md](../DESIGN.md) rests on.
- **Authors don't think about worlds.** `a + b` works whether or not either varies.
- **There's an independent test.** See [How it's checked](#how-its-checked).

**World-collapsing functions stay out of cells**, such as `expected(x)` or `possible(x > 5)`. `expected(total)` is one number, and narrowing can't narrow one number, so pinning a choice would show the old answer while really deciding would show a new one. Making it a macro doesn't change this, because the expansion would still collapse. Views read a finished value and can work these out after narrowing, which `Cond::expected`, `min` and `max` do.

### 2. Written as text, and the text is the only copy

A formula is parsed when it's written, and the parsed form is never saved. Text that doesn't parse is kept as written, the cell says why, it's reported on load, and a commit that writes it is refused. It heals when it's fixed.

### 3. Inputs are edges with a `label`

`label` is an edge component whose cell `called` holds a name. A formula's names resolve to the nodes its node links to by label edges with that name.

| Option | Why not |
|---|---|
| Node names in the text | Names aren't unique, renaming would break formulas, and the engine looks nothing up by name |
| Handles in the text | Unreadable |
| A binding table beside the formula | Two things that have to agree |
| **A `label` on an edge** | Chosen |

So:

- **A label can sit on an edge that's already there.** There's one edge per pair of nodes, holding several components, so a task's `depends_on` edge labelled `design` lets its formula say `design.finish` with no second edge.
- **A label alone is an edge.** Reading through it goes through the engine's ordinary reads and invalidation. Delete the target and the cell says `no input called rate`, and it heals when one is added.
- **Labels are useful beyond formulas.** The map can print them on edges.
- **Several edges with the same label make a group,** for `sum(line)`.

A pair of nodes has one edge and so one label, so a node can't be read under two names. That doesn't seem to cost anything, since a formula can use one name twice.

A label points from the formula to what it reads, so the formula and its inputs sit together in one file. That's the opposite of `part_of`, which points child to parent, so `label` says it arranges a map upside down. A total then sits above its lines, and a puzzle above its constraints above its choices.

### 4. Values and formulas are typed

| Type | Examples |
|---|---|
| `bool` | `a != b`. Stays a diagram, and is shown as a formula like `Stay=Hotel` |
| `number` | `prev * (1 + rate)` |
| `text` | A name |
| `node` | What a choice lands on, `car.chosen`. Its cells are read through it, as `car.chosen.price`, and two compare equal when they're the same node |
| `record`, `list` | Reading `rate.task.review.pages`, or a stored `value` with fields |

**Types are checked before any world is evaluated.** A type doesn't depend on a world. A node's `value` has its type in an internal cell of its own, `value_type`, worked out from the formula and its inputs' types, or from a stored value. So it's memoised, a value change doesn't rerun it because the type stays the same, and a loop is reported as a loop. A type error is an error in the cell, and says where in the formula.

**The cells compiled systems fill have their types listed in `formulas.rs`,** so `t.need` is `bool` and `t.finish` is `number`. Anything not listed is checked as it runs.

**What's read through a node is checked as it runs,** since which node it is differs between worlds, so `a.chosen.value = "rde"` isn't caught as a typo. A node's type could carry the nodes it might be, so that it could be.

**Whether a value is defined stays per world,** and isn't part of its type. See decision 6. A type like `number?` would force a check on every division for very little gain.

**Units are left room for, not built.** `number<days>` would catch adding days to pounds.

### 5. A formula can fill any cell that says it can hold one

`value` is the usual place for a node's answer, but a component's cell can hold a formula if its definition says how to compile one, and so can any of the workspace's own cells. That's what makes a plan's facts computable:

```yaml
components:
- task
cells:
  name: Global init search
formulas:
  estimate: if(chose(approach, gpu), 3, 5)
links:
- to: 01a1...            # Which approach?
  components:
  - label
  cells:
    called: approach
- to: 01a1...            # With a GPU
  components:
  - label
  cells:
    called: gpu
```

A typed-in estimate stays `estimate: 3`. `Worldly<T>` reads either a plain `T` or a `Cond<T>`, so a file stays plain when it can be.

An estimate that varies is read three ways. The schedule adds it per world, and an undefined one makes the dates unknowable, as a gap does. The `oversized_task` notice fires if it's over 5 days in any world, and says in which. The Work tab shows a range.

**Not every fact can be a formula.** What makes the worlds can't depend on them: a choice's `options`, a settled `chosen`, and which edges are `alternative`. A decision's options can depend on how other choices land, by an edge rather than a formula: see [Questions that come after others](#questions-that-come-after-others). A formula that's undefined in some world is an error for a cell that can't represent that, naming the worlds.

An estimate that reads a date which depends on that task is a loop, and is reported as one.

### 6. Undefined is a value in some worlds, not an error in the cell

Errors in a cell are all or nothing, but `1 / x` may only fail where `x = 0`, and `if(x != 0, 1 / x, 0)` has to be fine. So a value can be undefined in some worlds, and that spreads:

| Case | Result |
|---|---|
| `1 / 0`, `NaN`, a neighbour with no such cell, the literal `undefined` | Undefined in those worlds |
| Arithmetic or comparison with undefined | Undefined |
| `false && undefined` | False. It's three-valued, so `!(x > 5)` isn't quietly true when `x` is undefined |
| `if(undefined, a, b)` | Undefined |
| `defined(x)` | True or false, never undefined |

**This is also how "what's missing" comes through.** A total that depends on an open gap reads `undefined when Approach=Something else`, saying exactly what isn't known and where.

Errors in the cell are for what's wrong in every world: text that doesn't parse, a missing input, or a type error.

### 7. Functions are macros, and that includes the built-in ones

A function isn't called. It's expanded into the formula before evaluation, with its arguments substituted, so it holds no state and adds no new meaning. Only a small core is evaluated:

- Operators, `if`, `defined`, `has`, `chose`, `max`, `min`, `distinct`, and `round`, `floor` and `ceil`
- Group expansion, which turns `x in G: e` into one copy of `e` per node in `G`

The rest of the library is written in that core:

```
abs(x)                 = if(x < 0, -x, x)
or(x, fallback)        = if(defined(x), x, fallback)
effort()               = if(any(u in with.unknown: u.need), undefined, sum(t in with.task: if(t.need, t.estimate, 0)))
sum(x in G: e)         = e[x₁] + e[x₂] + … + 0
all(x in G: e)         = e[x₁] && e[x₂] && … && true
max(x in G: e, start)  = max(max(start, e[x₁]), e[x₂]) …
distinct(x in G: e)    = e[x₁] != e[x₂] && e[x₁] != e[x₃] && … && true, every pair
distinct(a, b, c)      = the same, over the values given
```

`product`, `any` and `count` are folds too. `count(line)` counts the nodes in a group, and `count(x in line: x > 0)` counts those where it's true.

`has(x.c)` is whether `x` has component `c`, if `c` names one, and otherwise whether there's anything in cell `c`. That's different from a cell being undefined: a task with no estimate and one whose estimate can't be worked out aren't the same thing. `max` and `min` over a group have to say where to begin, because an unknown could be the biggest, so undefined can't stand for an empty group. `whole.cell` reads a cell about the whole graph, such as `whole.plan_end`.

`sum(line)` is short for `sum(x in line: x)`. Group membership comes from the graph, so groups expand through ordinary reads, and a new line reruns the total. `all` keeps the order of its group, because the zebra puzzle's speed depends on clues being combined first.

**A macro of your own is a node:**

```yaml
components:
- macro
cells:
  name: Effort
  macro:
    params: [t]
    body: if(t.need, t.estimate, 0)
```

A formula labels an edge to it `effort`, and calls `effort(t)`. So it's in the log with an author and a reason, it can be reviewed in the inbox, and changing it reruns only what uses it.

- **Recursion needs no special check.** A macro's body can use macros it has label edges to. Its expanded form is an internal cell on its own node, `macro_body`, read by whatever calls it, so a caller only reads one hop. A macro that calls itself is a loop in those cells, which the engine already reports.
- **Bodies are closed.** A body can only use its parameters and other macros, so the same call means the same thing wherever it appears. The library is the exception: `effort()` has no parameters and scans for tasks.
- **Names can't capture.** Substitution is on the parsed form, so a macro's parameter can't clash with a name at the call site.
- **Types are checked after expansion,** with errors pointing at the call and the body. A macro works on anything its body does, so `max` works on numbers and text.
- **Size is capped.** `f(x) = g(x) + g(x)` nested 30 times is enormous, so expanding past a limit is an error in that cell. Identical subexpressions are evaluated once, which is safe because everything's pure.

Evaluation is guaranteed to finish: no recursion, finite groups, and a cap on size. The inspector can show a formula's expansion, which is the real explanation of what it does.

An expanded formula is a flat list, where each part's parts come first, so neither checking nor evaluating recurses, and how much memory it takes doesn't depend on how deep it goes. Nesting in the text is limited to 100 levels, so reading it can't run out of stack either.

### 8. A choice lands on a node, and what's read through it is that node's

A choice is a node with the `choice` component, whose `chosen` cell is the node it lands on in each world. A dot reads through it: `car.chosen.price` is the price of whichever car it lands on, world by world, and `car.chosen = tom` is the worlds where it's Tom. Writing one in settles it, `{"ref": handle}`, and taking that away opens it again. A choice doesn't say how it lands itself: what requires it does. A decision lands on its live options as likely as their weights say, and `options` on each of a plain list equally.

Options in a plan can have parts, dependencies, evidence and risks, and a budget's should be able to as well, so a budget's choices are ordinary decisions:

```yaml
# Train: an option. It has a price, and could have parts and dependencies too.
components:
- value
cells:
  name: Train
  value: 80.0
links:
- to: 01a1...            # Travel
  components:
  - alternative
```

```
Trip:    travel.chosen.value + stay.chosen.value      # 140, 180, 230 or 270
```

`Train`'s price is its `value`, and anything else can sit beside it, so a Plane option could be part of booking work, with the estimate and the cost varying together. Nothing is copied onto the decision: whatever an option says is read through it.

**`options` is the shorthand** for options with nothing to say, such as a puzzle's colours. Each option is still a node, but one nobody wrote: it's only its `name` and `value`, which are the option, and it's never saved. The same option makes the same node wherever it's listed, so `a.chosen = b.chosen` is the worlds where two regions are the same colour, and the zebra puzzle's houses are the same five nodes for every table. If an option later needs evidence, it becomes a decision.

## The language

```
expr     := or
or       := and ("||" and)*
and      := not ("&&" not)*
not      := "!" not | compare
compare  := sum (("=" | "!=" | "<" | "<=" | ">" | ">=") sum)?
sum      := term (("+" | "-") term)*
term     := unary (("*" | "/") unary)*
unary    := "-" unary | power
power    := atom ("^" unary)?
atom     := number | string | "true" | "false" | "undefined" | path | row | call | "(" expr ")"
path     := name ("." name)*
row      := name "[" expr "]" ("." name)*
call     := name "(" (name "in" path ":" expr ("," expr)? | expr ("," expr)*)? ")"
```

| Form | Means |
|---|---|
| `rate` | The `value` of the node labelled `rate` |
| `rate.finish`, `rate.task.review.pages` | Another cell on it, or a field of one |
| `self.need` | A cell on this node |
| `from.need`, `to.need` | On an edge, a cell on its source or target |
| `o.via.weight` | In a fold over edges, a cell on the edge `o` was reached along |
| `prev.balance`, `next.left`, `prev.prev.f` | A cell on the row before or after this one in its table: see [Tables](#tables) |
| `row` | Which row this is: its number, or its key |
| `Year[100].balance`, `Year[row - 1]`, `Car[Hatchback].price` | A cell on a row of a table, by its key |
| `rows` | The group of rows in this node's table |
| `in.part_of`, `out.depends_on` | The group of nodes at the other end of edges with that component |
| `with.task` | The group of nodes with that component |
| `whole.plan_end` | A cell on the graph as a whole |

- `=` between truths means "exactly when".
- A plain label must be exactly one node, and that's an error otherwise. As a group, it can be any number, including none, so a total with no lines yet is 0.

## Typing one

The Inspector and the Table have one field for a cell, as in a spreadsheet: `=price * 2` is a formula, and `5`, `true` or `hello` is a value. Clearing it takes away what's written, so the cell goes back to its kind or its default. A leading `'` means take the rest literally, for text that really starts with `=`. The `=` is only how it's typed: what's stored is `price * 2`, so a file or a proposal doesn't have one. Turning a formula back into a value takes its labels with it.

The Inspector asks which node each new name refers to. The Table can use the names a cell already has, and sends you to the Inspector for a new one.

Any part of a formula can be tried on a node with nothing changed, which is what hovering over a bracket shows, and what "what it expands to" uses.

## What it isn't for

`plan` and `schedule` stay as systems. Their cells are structural, they're defaults on every node with their components, and they have invariants. Formulas read their cells, and can fill some of them: dates and `done` are default formulas, and an estimate can be one.

| Tier | For | Example |
|---|---|---|
| **Formula** | One node's specific logic, with no build | `prev * (1 + rate)` |
| **Macro** | Logic shared between formulas, in the graph | `effort(t)` |
| **System** | Components across the whole graph, with invariants | `need`, `choice` |
| **Plugin** (not built) | Packaged components someone else wrote | A TypeScript system |

## How it fits the longer term

### Claims you can check, not just structure

Claude can say "this depends on that", and with formulas it can say "this has to come in under £200", in a form the app holds it to. It arrives as reviewable text through the usual path: `propose`, checked as if committed, then the inbox. An idea can go from coarse text, to structure, to a checkable claim, and each step has an author and a reason.

### The separate systems become one model

Because a plan's cells can be read and estimates can vary, the plan and its costs are the same graph. `effort()` is the plan's days in every world. On the handbook plan it's 6 days for the printed booklet, 8 for the PDF and 5 for the website. Shared work counts once, because it's a sum over tasks and not a rollup through parents, which would double-count anything with two parents. A task with no estimate makes the total undefined, which is honest, and `or(t.estimate, 0)` says otherwise. A gap in play makes it undefined too, since the gap adds no task.

Add a `requires` node saying `effort() <= 6`, and the PDF is ruled out, with a readable reason. A decision's estimate and its dates can depend on each other, so "the GPU option is faster, but only if we've bought the GPU" is a formula.

### Saying what's missing

Undefined per world and `no input called x` both say what isn't known yet. A notice could list formulas with undefined worlds and what they depend on, adding to the gaps `plan` reports. Claude could draft a formula first, and let what's missing drive the next question.

### Formulas explain themselves

A cell's formula and its expansion are its explanation, and the inspector shows what any part of it evaluates to. Claude can get the same breakdown over MCP.

### Plugins

Macros and formulas on any cell cover much of what people would write a plugin for. A component's defaults can be formulas, which is how dates and `done` are written, but only a system can declare a component. A workspace gets a column of its own from a kind or a table.

### Sharing between machines

Neutral, but it fails well. A merge can leave a formula and its labels disagreeing, and that's an error in that cell which heals, not corruption.

### Ingesting sessions and documents (speculative)

Text from past sessions often holds claims like "must be under 200ms" or "about three weeks". Extraction could propose these as formulas with `sources`, for review. Nothing here gets in its way.

## Risks

| Risk | What to do |
|---|---|
| **Distinct values explode.** `Cond<f64>` costs one case per distinct value, so a sum over 40 independent priced choices could need 2⁴⁰ cases | A hard cap, checked before each `zip`, gives a cell error naming the choices involved. Truths are safe from this, since they stay diagrams |
| **Reading other cells goes through JSON.** A `Bool` is rebuilt on the way in | `value` and the plan's and schedule's cells are read typed. Others, measure first |
| **A loop that exists only across worlds.** `if(c, a, b)` reads both sides, so a loop through the branch not taken is still reported | A limit of the engine. Formulas make it easier to hit, so the message should say so |
| **Float equality.** `0.1 + 0.2 = 0.3` is false | Exact, as documented, with `round` available |
| **Language creep.** Each feature looks small | Decision 1 and a small evaluated core are the test. A real need for more is a sign it should be a system |

The limits are 10,000 distinct values for one formula, 4,000,000 pairs for one step, and 200,000 steps to expand one formula.

## How it's checked

| Check | How |
|---|---|
| **The core property** | Random typed formulas in every world: the world-value evaluator against a plain evaluator with ordinary values, sharing only the parser |
| **The lens is honest** | For random formulas: reading then narrowing equals deciding through a real commit, then reading |
| **Macros mean what they expand to** | A formula with macros against the same formula written out by hand. A macro calling itself is reported as a loop |
| **Types** | Ill-typed formulas are rejected before any world is looked at, and a changed number doesn't rework any types downstream |
| **Estimates that vary** | The plain schedule with an estimate per world agrees in every world of random plans, and slack is still checked by actually slipping tasks |
| **Samples keep their answers** | 96, `Stay=Hotel`, 18, 92, 288, and the Japanese owning the zebra |
| **A held-out problem** | See below |

**The held-out test is the one that matters.** Every sample was written alongside the code, so none of them shows the library is enough. These are solved using only data, through `propose`, with no Rust:

1. A budget cap on the handbook plan, with estimates depending on a decision, which crosses systems.
2. A logic puzzle with a clause that isn't a pair: at most one of three people is lying.
3. A tiered price and a loan repayment, using a macro of its own.

If a new one needs Rust, that's the finding, and it goes in this document.

## Open questions

1. **Should `requires` narrow the plan's views?** Say a budget node rules out the PDF. It could dim that option on the Decisions tab and hide its work, or only show it on the Worlds tab with a notice saying why. It does only the Worlds tab today.
2. **Should a choice be able to read as its own answer?** `a.chosen.value` is explicit, but a puzzle says it a lot. A choice could say which of its cells read through to what it lands on, so `a` alone would mean `a.chosen.value`.

## Kinds

When several nodes are the same kind of thing, a node for the kind says what they have in common. Each links to it by `is_a`, and has its components and takes any cell it doesn't say for itself from it:

```
Petrol car:  cost_per_mile = fuel * 4.546 / self.mpg      (fuel labels Fuel price)
Hatchback:   is_a Petrol car, mpg 52, price 18000
Hybrid:      is_a Petrol car, cost_per_mile 0.09, price 26000
```

- **A value is taken as it is, and a formula runs for each node of that kind.** `self` is the hatchback, so it's the hatchback's mpg.
- **Names are labelled once, on the kind.** A formula taken from a kind follows label edges from where it's written, so `fuel` doesn't need an edge from every car.
- **A node's own word wins,** and taking it away brings the kind's back. The hybrid's maker gives its cost a mile, so it says that for itself.
- **A kind can have a kind.** What's more specific comes first, so a kind always comes before what it's a kind of.
- **What's about one node alone isn't taken.** A kind doesn't give its instances its `name` or its `value`, or make them `decision`s.

It's in the engine, rather than written as a default, because which components and cells a node has depends on its kind, and memos have to follow changes to the kind. `tests/kinds.rs` checks this against a newly built graph through random edits.

On a map, `is_a` is dashed and doesn't decide what goes above what. A kind is where a formula is written, not a step its numbers go through, so what the kind's formulas read goes beneath each node they run for: Fuel price sits under the petrol cars.

Whatever the option a decision lands on says is read through it: a constraint reads `car.chosen.range >= trip`, and "Yearly cost" reads `car.chosen.price`, `car.chosen.cost_per_mile` and `car.chosen.insurance`. A car's cost a mile comes from its kind, and that's what's read.

## Tables

A node named `Year[3]` is row 3 of the table `Year`, and one named `Car[Hatchback]` is the Hatchback row of `Car`. The name is all there is to it: nothing else is stored, so nothing can disagree with it, and renaming a row moves it. In the files, each row is an ordinary node.

A row takes what it doesn't say from the node named for its table, as a node does from its kind, so the table's components are every row's. A formula there is a column, like one filled down in a spreadsheet:

```
Rate:     0.05
Year:     balance = prev.balance * (1 + rate)      (rate labels Rate)
Year[0]:  balance 1000
Year[1] … Year[10]
```

- **A numbered row has neighbours.** `prev` is the row before, `next` the row after, and `prev.prev` the one before that. Before the first row there's nothing, so `prev` there is undefined, and the first row says its own value, as `Year[0]` does.
- **A numbered table goes on by itself.** Past its last written row, and in any gap, there's a row nobody wrote, which takes everything from the table. `Year[100].balance` works out each year up to it. Those rows exist only to be read: they're never saved, and they're the same each time they're asked for, so what's worked out on them stays in the memos. Reading one of the workspace's own cells the table doesn't say on one of them is an error, naming the row, since nothing could ever fill it. It isn't a node, so it has no name and only the components its table gives. So a table only writes the rows with something of their own: compound interest writes Year[0], where it starts, and Year[5], with its top-up, and Fibonacci writes F[0] and F[1]. They go up to 100,000 rows past the last written one.
- **A formula can address any row** as `Year[3]`, `Year[row - 1]` or `Car[Hatchback]`. A key has to be known without looking at any world, so it's a number, a name, or plain arithmetic on `row`. A table whose name has spaces can't be addressed this way yet.
- **`row` is which row this is,** a number for `Year[3]` and text for `Car[Hatchback]`, and `rows` is every row of the table, so `sum(rows.cost)` is a column's total.
- **Rows of related tables with the same key are one thing.** If Petrol car `is_a` Car, then `Car[Hatchback]` and `Petrol car[Hatchback]` are parts of one car. Each table holds only what applies to it: every car has a price, and only petrol cars have an mpg. A table says it has a cell even when it can't say what it comes to, as `Car` does with `price = undefined`, so its rows read as having it and the table lists it; each row says its own. Each part reads the cells it doesn't say from the others, and then from the tables of all of them. What's more specific wins: a part's own word, then Petrol car's, then Car's. A petrol car with no `Car` row is still a car.
- **A choice part way down changes only what comes after.** A row can say its own formula, such as `Year[5]` adding a top-up that's a decision. The rows before it have one value, and the rows after have one for each option.

In the app, the Table tab shows any table as a grid: a column for each cell the table and the kinds above it say, the table's own formulas on the top row, and what a row takes rather than says faded. Typing in a faded cell says it for that row alone. A numbered table shows its rows from the first to as far as you ask, with ◇ marking each row nobody wrote, which is read-only, and has a chart of a column down its rows, with a line for each answer to a choice the column waits on, so a choice at `Year[5]` shows where the lines part. On the map, a row is called by its key under its tables' names, and the parts of one thing are one card. An arrow goes from a node to each row it reads by where it is, as `prev` or `Year[30]`. A row nobody wrote is on the map, marked ◇, only when something written reads it: Year[4], because Year[5] reads it as `prev`, and Year[30], because "After 30 years" asks for it. What those rows read in turn doesn't come too.

The op `{"op": "table", "name": "Car", "is_a": .., "components": [..], "columns": {..}, "rows": [{"key": "Hatchback", ..}], "range": [0, 10], "alternative_of": ..}` writes a whole table at once. Rows that exist already are used, not made again. A decision's options are edges, each with its weight and whether it's ruled out, so `alternative_of` links every row, and adding a row in the grid links it as the others are.

`value` is each node's own answer, so a table's columns are cells of the workspace's own, such as `balance`. A table can be a table of choices: give the table `options`, and each row has them and is a choice. The table isn't a choice itself, since its options are what its rows choose from, and its own `value` can be a formula over them:

```
Queen:     options 1 … 8
           value = distinct(q in rows: q.chosen) && distinct(q in rows: q.chosen.value + q.row) && distinct(q in rows: q.chosen.value - q.row)
Queen[1] … Queen[8]
```

Each row is a choice of column, and `q.row` is a row's key, so the table's formula says no two queens share a column or a diagonal. The puzzles are written this way. The zebra puzzle has a table for each kind of thing, such as `Pet[Zebra]` and `Drink[Water]`, whose rows choose a house, and each clue is a formula naming the rows it's about, with nothing to label: `Nationality[English].chosen = Colour[Red].chosen`, or `abs(Smoke[Kools].chosen.value - Pet[Horse].chosen.value) = 1` for "next to". The sudoku's cells are a table too, and each row, column and box is a clue such as `distinct(Cell[A1].chosen, Cell[A2].chosen, Cell[A3].chosen, Cell[A4].chosen)`.

A formula anywhere can go over a table's rows by its name, as `count(v in Arrived.rows: v < 9.5)`, and find a row by the key of one it's going over, as `Left[v.row]`.

The engine keeps an index of names, so finding a table's rows or a row's neighbour doesn't mean reading every name. Renaming a node, linking a table to a kind, or removing a row reaches everything that could take a cell from somewhere else as a result. `tests/tables.rs` checks this against a newly built graph through random renames, kinds and edits, and `tests/toy_table.rs` in the systems checks the formulas.

## Reasoning under doubt

The Vane watch sample is a case file rather than a puzzle. "Who took it?" is a decision nobody makes, marked `uncertain`, whose options are the suspects, each weighted by how likely they seemed before the evidence. What's in doubt is the same: "When did Tom leave the Red Lion?" has two options, each an account with who gave it ("At 9:40, as the barman says", weight 3, and "At about 9:10, as Mrs Dale says", weight 1). A question's answer is which account is true, so what follows from it is a cell named for what it is, which each account says and which is read through the question: Tom's arrival is `pub.chosen.left_at + journey.chosen.minutes`. The evidence is two nodes that are required: `who.chosen.arrives <= 60` and `who.chosen.from_garden`.

So the worlds are every way the evening could have gone, weighted by the accounts, and the evidence rules some out. Wherever the app gives a percentage for something uncertain, on a map card, on the Decisions tab, in the bar at the top and on the Worlds tab, it's the belief in it: how strongly it's believed among the worlds left, its weight given the evidence and whatever's pinned or found out. So it changes as things are found out. Its prior belief is how strongly it was believed before any evidence: its weight, out of all its options' weights, which are cells on the `alternative` edges. Neither is a cell: a belief depends on what's pinned, and a prior belief is a share of its decision's weights, so both are worked out for each view. Where the evidence has moved a belief, the prior belief is shown beside it, so when two suspects both fit the evidence, it's plain their beliefs differ only because their prior beliefs did. A choice that's someone's to make isn't believed, so it shows neither. How likely a condition is, as in "true in 33%", is the same: among the worlds left. On the Decisions tab, something uncertain is "found out" rather than chosen. Pinning is finding something out. For each uncertain thing still open, it also says what's most worth finding out: each other open question, by how sure of the answer you'd be on average once you knew it. That's the chance of the likeliest answer given what the other turns out to be, weighted by how likely that is. At the start Tom is likeliest at 38%, and how Mr Pryce got home is most worth finding out (38% → 48%), while when Tom left the pub wouldn't change who's likeliest at all.

## Arguments

The philosophy samples use the same pieces for claims and arguments rather than chances. A claim is something uncertain that's accepted or rejected, an argument is a requirement on which claims can be held together, and the worlds left are the positions that are still coherent.

- **Free will** is three claims that each seem right and can't all be true. One requirement says so, which leaves seven of the eight ways to take them, and each way of giving up exactly one is a named position, written as a value that's true in that world. Pin any two as accepted and the third is rejected.
- **The trolley problem** has three principles, and a table of cases whose one formula says what the principles permit in each: `Case[Switch].permitted`. Two common judgements about cases are required, and only one set of principles fits them. What it says about a third case isn't put in: it follows. That's reflective equilibrium, with the worlds as the sets of principles and the judgements as evidence.
- **Reason or experience?** is a dialectic. Two opposite answers each say what they can explain, and what any answer has to explain is required: `or(answer.chosen.explains_necessity, true)`, so an answer is ruled out only if it says it can't. Each answer fails one requirement, and what's left is the gap, an answer nobody has named. Naming one that explains both closes it.

## Questions that come after others

Some questions can only be asked once others have an answer. In the Monty Hall sample, which door Monty opens depends on where the car is and which door you picked. Two edges say this, and neither changes a weight.

**`excludes`: whatever that choice lands on isn't an option here.** It goes from a decision to another choice:

```
Where's the car?              uncertain, one of Door[1], Door[2], Door[3]
Which door do you pick?       yours, one of the same three
Which door does Monty open?   uncertain, one of the same three, excludes the car's and yours
Which door is left?           one of the same three, excludes yours and Monty's
```

- **Each world normalises over what's left.** With the car behind your door, Monty has two doors at a half each. Otherwise he has one, and it's certain. The weights on the options are what they were: an option's chance is its weight out of the weights of what's left. Under the surface, each set of options that can be left has a variable of its own.
- **It isn't the same as a `requires`.** A rule saying "the door he opens isn't yours or the car's" keeps two worlds when the car is behind your door and one when it isn't, so it would make the car twice as likely to be behind your door as it should be, which is the famous wrong answer. A constraint rules out worlds; `excludes` says what there is to choose from.
- **It's what orders them.** A question comes after the ones it leaves out, and a map puts it above them, as it puts a formula above what it reads: what comes first is at the bottom, and what follows from it above. Two that leave out each other are a loop, and reported as one.
- **Pinning one is finding something out.** Pinning "Monty opens door 3" narrows to the worlds where that's what he does, which says something about the car. Settling it for good says the same, so a what-if and the real thing still can't disagree.
- **With nothing left, it lands on nothing,** as a decision with no options does.

**`within`: only what's part of whatever that choice lands on is an option here.** It also goes from a decision to another choice, and what belongs to what is said by `part_of`. In Bertrand's box, each coin is part of its box:

```
Which box?                       uncertain, one of the three boxes
Which coin do you take?          uncertain, one of the six coins, within the box
Which coin is left in the box?   one of the six coins, within the box, excludes the one you took
```

Everything said of `excludes` holds for it: each world normalises over what's left, it orders the questions, and pinning one is finding something out, so holding a gold coin says which box it probably is. Something can be part of several things, as a sock is in every load of washing big enough to have it. Where the other choice lands on nothing, nothing is within it. Together the two are drawing without putting back: each sock out is within the load, and excludes the socks already out.

What you end up with is a value like any other, which picks between two answers:

```
Stick or switch?      yours, with options Stick and Switch
The door you end on:  if(choice.chosen.value = "Switch", left.chosen, pick.chosen)
You win the car:      ends = car.chosen
```

Stick and Switch have nothing to say but their names, so the decision lists them in its `options`, as a puzzle's choice lists its colours, and there are no nodes or links for them. A decision can have both: options that are nodes linked to it, with weights, parts and evidence, and options it only names.

Which door you pick is yours too, so there's no one chance of winning to give: averaging over a choice still to be made means nothing. There is one for each way your choices could go, so the comparison on the Decisions tab gives the least and the most of those, and where they're the same, as two thirds is for Switch whichever door you pick, it says that.

On a map, an option of several questions that has nothing to say but what it's an option or a part of isn't a card: it's drawn on each question, to pin it there, so three doors asked about four times aren't twelve lines. Where those options are rows of a table with something to say, a cell of their own or what they're part of, the table has a card: a line for the rows that say the same in every column, with which rows they are set apart from what they say, since that isn't one of their cells. Clicking a column gives a line for each thing it says instead, and the other columns say what they come to across those rows.

What this doesn't do:

- **Options that aren't written down.** Every option is still a node with an `alternative` edge to each decision it's an option of, so three draws from eight socks is twenty-four edges. The edges say what could ever be chosen, and `within` and `excludes` what can be in each world.
- **The same options with different weights depending on something else.** A test that's positive 90% of the time if you have a condition and 5% if you don't is still a question for each case, as the positive test sample has it.
- **Pinning a choice of yours to something it couldn't always be** reads as finding out that it could. Stick and Switch are always there, so they don't have this.
- **What's shown isn't simplified by what's pinned.** A value reads right and is as likely as it says, but where it holds may still mention a world the pin has ruled out.

## Comparing options

Decision 1 keeps world-collapsing functions out of cells, so comparing options is the views' job. They do it from finished values, the same way pinning does:

- **Side by side.** Each open decision on the Decisions tab has a row per option. The columns are the options' own cells, and any value elsewhere that differs depending on which is taken. Each row is the view asked with that option pinned, so it can't disagree with really deciding.
- **Choices and chances.** A decision marked `uncertain` isn't anyone's to choose, like how far you'll drive, so its weights are how likely each option is. A view says which open choices each cell differs over. What's expected averages over uncertain ones only: averaging over a choice still to be made means nothing, so there it's a range, naming the choice it waits on.
- **How it changes.** `/api/sweep` gives some cells with another typed in as each of a list of values, on a copy of the graph that keeps its memos. The comparison charts a column against any number that isn't a choice, one line per option, so where one option overtakes another can be read off.

What this leaves out: an expected value can't be a cell, so "expected cost under £4,000" can't be a `requires`.
