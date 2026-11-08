# Expressions

**Status:** built. The language is `crates/recollect-systems/src/expr.rs`, the system is `formulas.rs`, and the tests are `tests/expr.rs`, the held-out problems in the server's tests, and the rest of the toys. [What changed in building it](#what-changed-in-building-it) is at the end.

## The problem

Everything a workspace can compute today is compiled Rust. To say "last year's balance times one plus the rate", someone wrote `sheet/year`, and to say "both of these people are knaves", someone wrote `puzzle/both_are`. So:

- Claude can arrange what exists, but it can't state a new relationship. That's the main limit on having an LLM in the loop.
- The library grows by one rule for each example: `Year`, `BothAre` and `Truthful` each have one user, and so does `NotAttacking`.
- A number has two types, `Amount` and `Cost`, depending on whether it varies between worlds, so `Sum` and `TotalCost` do the same job.
- A rule's parameters and its links say the same thing. A `Pair` holds `a` and `b`, and also has an `about` link to each.
- A plan's facts can't be worked out: a task's estimate is a number someone typed, and can't depend on a decision.

## The design in one example

```yaml
id: 01a1...
components:
  name: Year 3
rules:
  value:
    rule: expr
    params:
      text: prev * (1 + rate)
links:
- to: 01a1...            # Year 2
  components:
    label: prev
- to: 01a1...            # Rate
  components:
    label: rate
```

One rule, `expr`, can fill a typed cell. One link component, `label`, names what it reads.

## The decisions

### 1. A formula means its result in each world, separately

> For any world `w`, a formula's value in `w` is what you'd get by evaluating it on ordinary typed values, using its inputs' values in `w`.

The evaluator runs on world values, where `Cond::zip` and `Bool` make that exact and cheap. The meaning doesn't mention worlds at all. So:

- **The lens stays honest.** Narrowing a value and really deciding can't disagree, which is what [DESIGN.md](../DESIGN.md) rests on.
- **Authors don't think about worlds.** `a + b` works whether or not either varies, so there's no `Amount` and `Cost`.
- **There's an independent test.** See [How it's checked](#how-its-checked).

**World-collapsing functions stay out of cells**, such as `expected(x)` or `possible(x > 5)`. `expected(total)` is one number, and narrowing can't narrow one number, so pinning a choice would show the old answer while really deciding would show a new one. Making it a macro doesn't change this, because the expansion would still collapse. Views read a finished value and can work these out after narrowing, which `Cond::expected`, `min` and `max` already do.

### 2. Written as text, and the text is the only copy

The parameter is the formula's text, parsed when the rule is built, and the parsed form is never saved. Text that doesn't parse is `Error::Malformed`: it's kept as written, reported on load, and heals when fixed.

### 3. Inputs are links with a `label`

A `label` is a link component holding a name, and a formula's names resolve to the node's outgoing links with that label.

| Option | Why not |
|---|---|
| Node names in the text | Names aren't unique, renaming would break formulas, and the core looks nothing up by name |
| Handles in the text | Unreadable |
| A binding table in the parameters | The duplication `Pair` has: the rule needs links in order to read, so the two have to agree |
| A separate `reads { as: rate }` kind | The first draft. It isn't needed, and it stops other links being read |
| **A `label` on any link** | Chosen |

This works because a link is already a bag of components. So:

- **A label can sit on a link that's already there.** A task's `depends_on` link labelled `design` lets its formula say `design.finish`, with no second link saying the same thing.
- **A label alone is a link.** The engine treats it as a kind, so a rule declares `outgoing("label")` and reads through the existing checks and invalidation. Deleting the target removes the link, so the cell says `no input called rate` and heals when one is added.
- **Labels are useful beyond formulas.** The map can print them on edges, and any link can be named.
- **Several links with the same label make a group,** for `sum(line)`.

A pair of nodes has one link and so one label, so a node can't be read under two names. That doesn't seem to cost anything, since a formula can use one name twice.

A label-only link points from the formula to what it reads, so the formula and its inputs sit together in one file. That's the opposite of "links point child to parent" on the map, so the catalog needs to say that `label` arranges upside down. A total then sits above its lines, and a puzzle above its constraints above its choices. If a label is on a link that already has an arranging kind, that kind decides, or the layout would get two opposite edges for one link.

### 4. Values and formulas are typed

The first draft said "plain numbers". That was loose wording for dynamically typed scalars, and it should be properly typed.

| Type | Examples |
|---|---|
| `bool` | `a != b`. Stays a diagram, and is shown as a formula like `Stay=Hotel` |
| `number` | `prev * (1 + rate)` |
| `text` | A name |
| `enum` | A `Domain`'s options, so `Milk = "3"` can be an error if its options are numbers, and `Colour = "rde"` is caught as a typo |
| `record`, `list` | Reading `rate.task.review.pages`, or a stored `value` with fields |

**Types are checked before any world is evaluated.** A type doesn't depend on a world, so each cell's type is its own derived column, worked out from the formula and its inputs' types. That reuses everything the engine already does: it's memoised, a value change doesn't rerun it because the type stays the same, and a loop is reported as a loop. A type error is an error in the cell, and it says where in the formula. It can't depend on which worlds are open.

**A node can declare a type, or have one inferred.** A stored `value: 1000` is a number, and `type: number` on a formula is checked against what's inferred. A `Domain` is an enum.

**Compiled components have their types listed in `formulas.rs`,** so that `t.need` is `bool` and `t.finish` is `number`. Anything not listed is checked as it runs.

**Whether a value is defined stays per world,** and isn't part of its type. See decision 6. So a number is a number, and might be undefined in some worlds. A type like `number?` would force a check on every division for very little gain.

**Units are left room for, not built.** `number<days>` would catch adding days to pounds, and fits the type column when it's wanted.

### 5. A formula can fill any typed cell, not only `value`

This is what makes a plan's facts computable. `value` is the default place for a node's answer, and it replaces `amount`, `cost`, `landing`, `holds`, `valid` and `statement`, but `expr` also works on a component that says it can hold a formula.

A task's estimate moves out of `Task` into its own cell, because a rule fills a cell and not a field:

```yaml
components:
  name: Global init search
  task: null
rules:
  estimate:
    rule: expr
    params:
      text: if(chose(approach, gpu), 3, 5)
links:
- to: 01a1...            # Which approach?
  components:
    label: approach
- to: 01a1...            # With a GPU
  components:
    label: gpu
```

A typed-in estimate stays `estimate: 3`.

This has three consequences:

**The engine changed a little.** `Registry::resolve` refuses a rule on any component but what it provides, which is a good protection. `reg.rule_on::<C>(key, build)` says a key can also fill `C`, so `expr@1` is still refused on anything that hasn't said so, with no new check needed. A log's record of the code lists them.

**Stored and worked-out values need one shape.** A new `Worldly<T>` reads either a plain `T` or a `Cond<T>`, so a file stays plain when it can be. `value` and `estimate` both use it.

**The readers of an estimate have to handle one that varies.** There are three:

| Reader | Today | Then |
|---|---|---|
| `schedule::own_duration` | `f64` | `Cond<Option<f64>>`, so `s + d` is a `zip`. Undefined makes the date unknowable, as a gap does, and no estimate is still 0 |
| `oversized_task` notice | `d > 5` | Fires if it's over 5 in any world, and says in which |
| The Work tab | One number | A range, which `casesText` already gives |

**Not every fact can be a formula.** What makes the worlds themselves can't depend on them: `Domain`, a decision's options, `Alternative`. A component opts in only if varying per world makes sense for it, and a formula that's undefined in some world is an error for a component that can't represent that, naming the worlds.

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

- Operators, `if`, `defined`, `has`, `chose`, `max`, `min`, and `round`, `floor` and `ceil`
- Group expansion, which turns `x in G: e` into one copy of `e` per node in `G`

The rest of the library is written in that core:

```
abs(x)                 = if(x < 0, -x, x)
or(x, fallback)        = if(defined(x), x, fallback)
sum(x in G: e)         = e[x₁] + e[x₂] + … + 0
all(x in G: e)         = e[x₁] && e[x₂] && … && true
max(x in G: e, start)  = max(max(start, e[x₁]), e[x₂]) …
distinct(x in G: e)    = e[x₁] != e[x₂] && e[x₁] != e[x₃] && … && true, every pair
distinct(a, b, c)      = the same, over the values given
pick(o in G: e)        = if(chose(self, o₁), e[o₁], if(chose(self, o₂), e[o₂], … undefined))
pick(o in G: e, other) = the same, with other in place of undefined
```

`has(n.cell)` says whether a node has a cell at all, which is different from that cell being undefined: a task with no estimate and one whose estimate can't be worked out aren't the same thing. `max` and `min` over a group have to say where to begin, because an unknown could be the biggest, so undefined can't stand for an empty group. `whole.cell` reads a cell about the whole graph, which only a component's default can fill.

`sum(line)` is short for `sum(x in line: x)`. Group membership comes from the graph, so groups expand at evaluation through ordinary reads, and a new line reruns the total. `all` keeps the order of its group, because the zebra puzzle's speed depends on clues being combined first.

**A macro of your own is a node:**

```yaml
components:
  name: Effort
  macro:
    params: [t]
    body: if(t.need, t.estimate, 0)
```

A formula labels a link to it `effort`, and calls `effort(t)`. So it's in the log with an author and a reason, it can be reviewed in the inbox, and changing it reruns only what uses it.

- **Recursion needs no special check.** A macro's body can use macros it has links to. Its expanded form is a cell on its own node, read by whatever calls it. That also keeps rules local, since a caller only reads one hop. A macro that calls itself is a loop in those cells, which the engine already reports.
- **Bodies are closed.** A body can only use its parameters and other macros, so the same call means the same thing wherever it appears.
- **Names can't capture.** Substitution is on the parsed form, so a macro's parameter can't clash with a name at the call site.
- **Types are checked after expansion,** with errors pointing at the call and the body. A macro works on anything its body does, so `max` works on numbers and text.
- **Size is capped.** `f(x) = g(x) + g(x)` nested 30 times is enormous, so expanding past a limit is an error in that cell. Identical subexpressions are evaluated once, which is safe because everything's pure.

Evaluation is still guaranteed to finish: no recursion, finite groups, and a cap on size. The inspector can show a formula's expansion, which is the real explanation of what it does.

### 8. Options are nodes, wherever an option has anything to say

Options in a plan can have parts, dependencies, evidence and risks, and a budget's should be able to as well, so a budget's choices are ordinary decisions:

```yaml
# Train: an option. It has a price, and could have parts and dependencies too.
components:
  name: Train
  value: 80
links:
- to: 01a1...            # Travel
  components:
    alternative: {}
```

```
Travel:  pick(in.alternative)      # 80 or 120
Trip:    travel + stay
```

`Train`'s price is its `value`, and anything else can sit beside it, so a Plane option could be `part_of`-linked to booking work, with the estimate and the cost varying together. Trip budget then lists `plan` among its systems, gets a Decisions tab, and `OptionOf` and `ChoiceCost` go. The existing answer, over £200 exactly when `Stay=Hotel`, has to still hold.

**`Domain` stays,** as a shorthand for options with nothing to say. Both make the same kind of world variable, and differ only in whether an option is a node. Eight queens would otherwise need 64 option nodes and the zebra puzzle 125, none of them with anything of their own. If an option later needs evidence, it becomes a decision. Whether that's consistent enough is [question 2](#open-questions).

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
| `prev.balance`, `next.left`, `prev.prev.f` | A cell on the row before or after this one in its table: see [Tables](#tables) |
| `row` | Which row this is: its number, or its key |
| `Year[100].balance`, `Year[row - 1]`, `Car[Hatchback].price` | A cell on a row of a table, by its key |
| `rows` | The group of rows in this node's table |
| `in.part_of`, `out.depends_on` | The group of nodes linked by that kind |
| `with.task` | The group of nodes holding that component, using the engine's declared scans |

- `=` between truths means "exactly when", which is what `Truthful` is.
- A plain label must be exactly one node, and that's an error otherwise. As a group, it can be any number, including none, so a total with no lines yet is 0.

## Typing one

The Inspector and the Table have one field for a cell, as in a spreadsheet: `=price * 2` is a formula, and `5`, `true` or `hello` is a value. Clearing it removes the cell, and a leading `'` means take the rest literally, for text that really starts with `=`. The `=` is only how it's typed: what's stored is `price * 2`, so a file or a proposal doesn't have one. Turning a formula back into a value takes its labels with it.

The Inspector asks which node each new name refers to. The Table can use the names a cell already has, and sends you to the Inspector for a new one.

## The samples, rewritten

| Sample | Today | With expressions |
|---|---|---|
| Invoice | `Product`, `Sum`, `Factor`, `Addend` | `quantity * price`, `sum(line)`, `subtotal * rate`, `subtotal + tax` |
| Fibonacci | `Sum` | `a + b` |
| Compound interest | `Year`, `Prev`, `Rate` | `prev * (1 + rate)` |
| Trip budget | `ChoiceCost`, `TotalCost`, `OptionOf` | Option nodes with a `value`, `pick(in.alternative)`, `travel + stay`, `trip > 200` |
| Australia, sudoku | `Pair(Different)` | `a != b` |
| Eight queens | `Pair(NotAttacking(n))` | `a != b && abs(a - b) != 3` |
| Zebra | `Pair(Same, Offset, Adjacent)`, `Is` | `a = b`, `a = b + 1`, `abs(a - b) = 1`, `milk = 3` |
| Knights and knaves | `BothAre`, `Truthful` | `a = "knave" && b = "knave"`, `(speaker = "knight") = said` |
| A puzzle's solutions | `AllHold`, `Constrains` | `all(constraint)` on a node marked `requires` |
| Calibration, critical path | `plan` and `schedule` | Unchanged, apart from `estimate` being its own cell |

**That removes 10 rules and 14 components:** `Sum`, `Product`, `Year`, `ChoiceCost`, `TotalCost`, `AllHold`, `Pair` and its `Relation`, `Is`, `BothAre` and `Truthful`, and `Amount`, `Cost`, `Landing`, `Holds`, `Valid`, `Statement`, `Addend`, `Factor`, `Prev`, `Rate`, `OptionOf`, `About`, `Constrains` and `Says`. They become `expr`, `value`, `label`, `macro` and `requires`, and `ChoiceOf`, `Domain` and `At` stay. `sheet` and `puzzle` would be one system.

**`requires`** is a marker meaning "only the worlds where this node's value is true count". It's the general form of today's `Catalog::constraint::<Valid>`.

## What it isn't for

`plan` and `schedule` stay compiled. Their rules are structural and run down every node as defaults, they have invariants, and they're checked by 268,905 node-world comparisons. Formulas read their cells, and can now feed some of them.

| Tier | For | Example |
|---|---|---|
| **Formula** | One node's specific logic, with no build | `prev * (1 + rate)` |
| **Macro** | Logic shared between formulas, in the graph | `effort(t)` |
| **System** | Structure across the whole graph, with invariants | `need`, `choice` |
| **Plugin** (not built) | Packaged components and rules someone else wrote | A TypeScript system |

## How it fits the longer term

### Claims you can check, not just structure

Today Claude can say "this depends on that". With formulas it can say "this has to come in under £200", in a form the app holds it to, and it arrives as reviewable text through the existing path: `propose`, checked as if committed, then the inbox. An idea can go from coarse text, to structure, to a checkable claim, and each step has an author and a reason.

### The separate systems become one model

Because a plan's cells can be read and estimates can vary, the plan and its costs are the same graph:

```
sum(t in with.task: if(t.need, t.estimate, 0))
```

That's the plan's effort in every world. On Calibration it's 6 days for CAD, 8 for free-form and 5 for the checkerboard, worked by hand. Shared work counts once, because it's a sum over tasks and not a rollup through parents, which would double-count anything with two parents. A task with no estimate makes the total undefined, which is honest, and `or(t.estimate, 0)` says otherwise.

"Something else" gives 1 day, which is wrong, because the gap adds no task. So it's really:

```
if(any(u in with.unknown: u.need), undefined, sum(...))
```

A user would have to know that, which is what a shared macro is for. It should ship in the library as `effort`, not be left for each workspace to discover.

Add a `requires` node saying `effort <= 6`, and free-form is ruled out, with a readable reason. A decision's estimate and its dates can now depend on each other, so "the GPU option is faster, but only if we've bought the GPU" is a formula.

### Saying what's missing

Undefined per world and `no input called x` both say what isn't known yet. A notice could list formulas with undefined worlds and what they depend on, adding to the gaps `plan` reports. Claude could draft a formula first, and let what's missing drive the next question.

### Formulas explain themselves

A cell's formula and its expansion are its explanation. With a parsed formula, the inspector can show each subexpression's value, and the bracket highlighting already marks what's under the mouse, so it could also show what that part evaluates to. Claude can get the same breakdown over MCP.

### The audit gets stronger

`graph-core.md` says "rule bodies can't be recorded", which is what limits the audit. A formula's body and any macro it uses are in the log, so a cell's whole meaning is written down, with `blame` and `undo` working as usual. What's left is the core language, so the key should be versioned as `expr@1`. A change in meaning gets a new key, and witnesses catch mistakes in that.

### Plugins

Macros and any-cell formulas cover much of what people would write a plugin for. Defaults can be formulas now, which is how dates and `done` are written, but only a system can set one so far. A workspace adding its own column is a small step from here.

### Sharing between machines

Neutral, but it fails well. A merge can leave a formula and its labels disagreeing, and that's an error in that cell which heals, not corruption.

### Ingesting sessions and documents (speculative)

Text from past sessions often holds claims like "must be under 200ms" or "about three weeks". Extraction could propose these as formulas with `sources`, for review. I wouldn't design for this yet, but nothing here gets in its way.

## Risks

| Risk | What to do |
|---|---|
| **Distinct values explode.** `Cond<f64>` costs one case per distinct value, so a sum over 40 independent priced choices could need 2⁴⁰ cases. Formulas make this easy to hit, and estimates that vary make dates worse | A hard cap, checked before each `zip`, that gives a cell error naming the choices involved. It needs measuring. Truths are safe from this, since they stay diagrams |
| **Estimates that vary touch tested code.** The schedule is checked against plain evaluators, and `Task` is used across tests | Extend the plain schedule to take an estimate per world. Moving the field is mechanical, and the old-kernel comparison doesn't use estimates |
| **The engine's output check is loosened.** It's a protection against rules on the wrong component | Only `expr`, and a catalog check takes its place |
| **Slower puzzles.** Zebra took 16 ms with compiled rules | It takes 31 ms now, 8 queens 8 ms against 3, and sudoku 5 ms against 3. A shortcut for when nothing's undefined took zebra from 80 ms to that |
| **Reading other components goes through JSON.** A `Bool` is rebuilt on the way in | Typed reads for `value`. For `need`, `finish` and so on, measure first |
| **A loop that exists only across worlds.** `if(c, a, b)` reads both sides, so a loop through the branch not taken is still reported | An existing limit. Formulas make it easier to hit, so the message should say so |
| **Float equality.** `0.1 + 0.2 = 0.3` is false | Exact, as documented, with `round` available |
| **Language creep.** Each feature looks small | Decision 1 and a small evaluated core are the test. A real need for more is a sign it should be a system |

## How it's checked

| Check | How |
|---|---|
| **The core property** | Random typed formulas in every world: the world-value evaluator against a plain evaluator with ordinary values, sharing only the parser and expander |
| **The lens is honest** | For random formulas: reading then narrowing equals deciding through a real commit, then reading |
| **Macros mean what they expand to** | A formula with macros against the same formula written out by hand. A recursive macro is reported as a loop |
| **Types** | Well-typed random formulas never give a type error at evaluation. Deliberately ill-typed ones are rejected before any world is looked at |
| **Estimates that vary** | The plain schedule with an estimate per world agrees in every world of random plans, and slack is still checked by actually slipping tasks |
| **Samples keep their answers** | 96, `Stay=Hotel`, 18, 92, 288, and the Japanese owning the zebra stay as they are, checked on both forms while both exist |
| **Round trips** | Saving, reloading and undoing give the same cells, in the existing random-edit comparison |
| **The checks can fail** | Deliberate breakage: dropping the spread of undefined, flipping an operand, wrong substitution, a skipped type check |
| **A held-out problem** | See below |

**The held-out test is the one that matters.** Every current sample was written alongside its rules, so none of them shows the library is enough. Before calling this done, solve new problems using only data, through `propose`, with no Rust:

1. A budget cap on the Calibration plan, with estimates depending on a decision, which crosses systems.
2. A logic puzzle with a clause that isn't a pair, such as "at most one of these three".
3. A tiered price, or a loan repayment, using a macro of its own.

If one needs new Rust, that's the finding, and it goes in this document.

## Open questions

1. **Should `requires` narrow the plan's views?** Say a budget node rules out free-form. It could dim that option on the Decisions tab and hide its work, or only show it on the Worlds tab with a notice saying why. It does only the Worlds tab today.
2. **Is keeping `Domain` consistent enough?** It's the one place an option isn't a node. The alternative is making every option a node and having the map collapse them, which costs 125 nodes on the zebra puzzle.
3. **Should a workspace be able to make a whole column?** A system can, with `formulas::defaults_to`. A workspace does it with a kind or a table: see [Kinds](#kinds) and [Tables](#tables). What neither can yet do is give its rows a system's cells, such as a `value`, or making each one a task.

## Kinds

A workspace's own cells, like a car's `price` or `range`, are any components no system declares, fills or gives a default to. A formula can fill one like any other cell, and its type is checked as it runs.

When several nodes are the same kind of thing, a node for the kind says what they have in common. Each links to it by `is_a`, and takes any of those cells it doesn't say for itself from it:

```
Petrol car:  cost_per_mile = fuel * 4.546 / self.mpg      (fuel labels Fuel price)
Hatchback:   is_a Petrol car, mpg 52, price 18000
Hybrid:      is_a Petrol car, cost_per_mile 0.09, price 26000
```

- **A value is taken as it is, and a formula runs for each node of that kind.** `self` is the hatchback, so it's the hatchback's mpg.
- **Names are labelled once, on the kind.** A formula taken from a kind may follow links from where it's written as well as from the node it runs for, so `fuel` doesn't need a link from every car.
- **A node's own word wins,** and taking it away brings the kind's back, as for a default. The hybrid's maker gives its cost a mile, so it says that for itself.
- **A kind can have a kind.** What's more specific comes first, so a kind always comes before what it's a kind of. Otherwise it's the nearest, and then the order the kinds were made. A loop of kinds goes round once and stops.
- **What a system owns stays the system's.** A kind can't give its instances a name, make them done or make them tasks.

It's in the engine, rather than written as a default rule, because which cells a node has depends on its kind, and memos have to follow changes to the kind. Changing a kind's cell reaches everything that takes it, however far down, and linking or unlinking a kind reaches every cell the node could take from it. `tests/kinds.rs` checks this against a newly built graph through random edits.

On a map, `is_a` is dashed and doesn't decide what goes above what. A kind is where a formula is written, not a step its numbers go through, so what the kind's formulas read goes beneath each node they run for: Fuel price sits under the petrol cars.

A decision can give any cell of whichever option it lands on: `range` on "Which car?" is `pick(o in in.alternative: o.range)`, and a constraint reads `car.range >= trip`. That's how a car's figures reach what's worked out from the one chosen: "Yearly cost" reads `car.price`, `car.cost_per_mile` and `car.insurance` the same way.

## Tables

A node named `Year[3]` is row 3 of the table `Year`, and one named `Car[Hatchback]` is the Hatchback row of `Car`. The name is all there is to it: nothing else is stored, so nothing can disagree with it, and renaming a row moves it. In the files, each row is still an ordinary node.

A row takes the workspace's own cells it doesn't say from the node named for its table, as a node takes them from its kind. A formula there is a column, like one filled down in a spreadsheet:

```
Rate:     0.05
Year:     balance = prev.balance * (1 + rate)      (rate labels Rate)
Year[0]:  balance 1000
Year[1] … Year[10]
```

- **A numbered row has neighbours.** `prev` is the row before, `next` the row after, and `prev.prev` the one before that. Before the first row there's nothing, so `prev` there is undefined, and the first row says its own value, as `Year[0]` does.
- **A numbered table goes on by itself.** Past its last written row, and in any gap, there's a row nobody wrote, which takes every cell from the table. `Year[100].balance` works out each year up to it. Those rows exist only to be read: they're never saved, and they're the same each time they're asked for, so what's worked out on them stays in the memos. Reading a cell the table doesn't say on one of them is an error, naming the row, since nothing could ever fill it. It isn't a node, so the cells every node gets by default aren't its either. So a table only writes the rows with something of their own: compound interest writes Year[0], where it starts, and Year[5], with its top-up, and Fibonacci writes F[0] and F[1]. They go up to 100,000 rows past the last written one.
- **A formula can address any row** as `Year[3]`, `Year[row - 1]` or `Car[Hatchback]`. A key has to be known without looking at any world, so it's a number, a name, or plain arithmetic on `row`. A table whose name has spaces can't be addressed this way yet.
- **`row` is which row this is,** a number for `Year[3]` and text for `Car[Hatchback]`, and `rows` is every row of the table, so `sum(rows.cost)` is a column's total.
- **Rows of related tables with the same key are one thing.** If Petrol car `is_a` Car, then `Car[Hatchback]` and `Petrol car[Hatchback]` are parts of one car. Each table holds only what applies to it: every car has a price, and only petrol cars have an mpg. Each part reads the cells it doesn't say from the others, and then from the tables of all of them. What's more specific wins: a part's own word, then Petrol car's, then Car's. A petrol car with no `Car` row is still a car.
- **A choice part way down changes only what comes after.** A row can say its own formula, such as `Year[5]` adding a top-up that's a decision. The rows before it have one value, and the rows after have one for each option.

In the app, the Table tab shows any table as a grid: a column for each cell the table and the kinds above it say, the table's own formulas on the top row, and what a row takes rather than says faded. Typing in a faded cell says it for that row alone. A numbered table shows its rows from the first to as far as you ask, with ◇ marking each row nobody wrote, which is read-only, and has a chart of a column down its rows, with a line for each answer to a choice the column waits on, so a choice at `Year[5]` shows where the lines part. On the map, a row is called by its key under its tables' names, and the parts of one thing are one card. An arrow goes from a node to each row it reads by where it is, as `prev` or `Year[30]`. A row nobody wrote is on the map, marked ◇, only when something written reads it: Year[4], because Year[5] reads it as `prev`, and Year[30], because "After 30 years" asks for it. What those rows read in turn doesn't come too.

The op `{"op": "table", "name": "Car", "is_a": .., "columns": {..}, "rows": [{"key": "Hatchback", ..}], "range": [0, 10], "alternative_of": ..}` writes a whole table at once. Rows that exist already are used, not made again. A decision's options are still links, since each says how likely it is or why it was ruled out, so `alternative_of` links every row, and adding a row in the grid links it as the others are.

What a system owns still can't come from a table, and that includes `value`, so a table's columns are cells of the workspace's own, such as `balance`. The one exception is a choice's options, its `domain`: a row takes its table's, as an instance takes its kind's, so a table can be a table of choices. The table isn't a choice itself, since its options are what its rows choose from, and its own `value` can be a formula over them:

```
Queen:     domain 1 … 8
           value = distinct(rows) && distinct(q in rows: q + q.row) && distinct(q in rows: q - q.row)
Queen[1] … Queen[8]
```

Each row is a choice of column, and `q.row` is a row's key, so the table's formula says no two queens share a column or a diagonal. The puzzles are written this way. The zebra puzzle has a table for each kind of thing, such as `Pet[Zebra]` and `Drink[Water]`, whose rows choose a house, and each clue is a formula naming the rows it's about, with nothing to label: `Nationality[English] = Colour[Red]`, or `abs(Smoke[Kools] - Pet[Horse]) = 1` for "next to". The sudoku's cells are a table too, and each row, column and box is a clue such as `distinct(Cell[A1], Cell[A2], Cell[A3], Cell[A4])`. That's 45 nodes for the zebra puzzle rather than 90, 10 for eight queens rather than 37, and 31 for the sudoku rather than 74, and the solutions are the same.

A formula anywhere can go over a table's rows by its name, as `count(v in Arrived.rows: v < 9.5)`, and find a row by the key of one it's going over, as `Left[v.row]`.

## Reasoning under doubt

The Vane watch sample is a case file rather than a puzzle. "Who took it?" is a decision nobody makes, marked uncertain, whose options are the suspects, each weighted by how likely they seemed before the evidence. What's in doubt is the same: "When did Tom leave the Red Lion?" has two options, each an account with who gave it ("At 9:40, as the barman says", weight 3, and "At about 9:10, as Mrs Dale says", weight 1). When each suspect could have reached the house is a formula over those, as Tom's `left + travel`. The evidence is two nodes that are required: `who.arrives <= 60` and `who.from_garden`.

So the worlds are every way the evening could have gone, weighted by the accounts, and the evidence rules some out. The Worlds tab gives each option's chance among what's left, which is its weight given the evidence and whatever's pinned. Pinning is finding something out. For each uncertain thing still open, it also says what's most worth finding out: each other open question, by how sure of the answer you'd be on average once you knew it. That's the chance of the likeliest answer given what the other turns out to be, weighted by how likely that is. At the start Tom is likeliest at 38%, and how Mr Pryce got home is most worth finding out (38% → 48%), while when Tom left the pub wouldn't change who's likeliest at all.

The engine keeps an index of names, so finding a table's rows or a row's neighbour doesn't mean reading every name. Renaming a node, linking a table to a kind, or removing a row reaches everything that could take a cell from somewhere else as a result. `tests/tables.rs` checks this against a newly built graph through random renames, kinds and edits, and `tests/toy_table.rs` in the systems checks the formulas.

## Comparing options

Decision 1 keeps world-collapsing functions out of cells, so comparing options is the views' job. They do it from finished values, the same way pinning does:

- **Side by side.** Each open decision on the Decisions tab has a row per option. The columns are the options' own cells, and any value elsewhere that differs depending on which is taken. Each row is the view asked with that option pinned, so it can't disagree with really deciding.
- **Choices and chances.** A decision marked `uncertain` isn't anyone's to choose, like how far you'll drive, so its weights are how likely each option is. A view says which open choices each cell differs over. What's expected averages over uncertain ones only: averaging over a choice still to be made means nothing, so there it's a range, naming the choice it waits on.
- **How it changes.** `/api/sweep` gives some cells with another typed in as each of a list of values, on a copy of the graph that keeps its memos. The comparison charts a column against any number that isn't a choice, one line per option, so where one option overtakes another can be read off.

What this leaves out: an expected value can't be a cell, so "expected cost under £4,000" can't be a `requires`.

## What changed in building it

- **A type column needed a small engine addition.** A type rule has to read whatever the formula reads, and what that is depends on the formula. `Inputs::like` says a rule may follow what the rule on another of its node's components may, and `cx.own_rule` reads how that rule is written.
- **An expanded formula is a flat list, not a tree.** A test found that a very large expansion overflowed the stack in the first version, and a long `sum` could have too. Checking and evaluating now walk a list where each part's parts come first, so neither recurses, and how much memory it takes doesn't depend on how deep it goes.
- **`count(line)` counts the nodes in a group,** and `count(x in line: x > 0)` counts those where it's true.
- **Nesting in the text is limited to 100 levels,** so reading it can't run out of stack.
- **The library can follow links, which a workspace's macros can't.** `effort()` has no parameters and scans for tasks, and `inputs_of` looks inside the library to find that. A workspace's macro only has its parameters.
- **Any part of a formula can be tried** on a node with nothing changed, which is what hovering over a bracket shows, and what "what it expands to" uses. It runs on a fork of the graph.
- **Working copies of samples have a format.** One from before samples last changed is moved aside to `~/.recollect/samples-superseded`, not misread. An old `task: {estimate: 3}` would otherwise silently lose its estimate.

The limits are 10,000 distinct values for one formula, 4,000,000 pairs for one step, and 200,000 steps to expand one formula.
