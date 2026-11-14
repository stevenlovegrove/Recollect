# Distributions

**Status:** all six steps are built, each with what it leaves out: [Order of building](#order-of-building) says which is which, and [What's built](#whats-built) what those leave out. The engine's part is `crates/recollect-graph/src/chance.rs`, the formula language's is in `expr.rs` and `formulas.rs`, and the tests are `tests/chance.rs` in `recollect-systems`. Five samples are written this way: see [The samples](#the-samples). For the worlds this extends, see [components.md](components.md#worlds-on-top), and for formulas, [expressions.md](expressions.md).

## Why

A choice with weights says "one of these, this likely each". That covers who took the watch, and it doesn't cover "about three weeks", "5% give or take 1%", or the roll of a die added to another. Today an estimate is one number, a rate is one number, and anything uncertain that isn't a short list has to be made into one.

## The idea: what's uncertain is part of the world

**A world is one way every open choice lands, and one draw of every uncertain quantity.** A distribution isn't a kind of value that formulas combine. It's a quantity that has one ordinary value in each world, as a choice has one option in each.

- **A formula still means what it did.** Decision 1 of [expressions.md](expressions.md) is unchanged: in any world, a formula's value is what ordinary evaluation gives on its inputs' values in that world.
- **What depends on the same thing stays related.** `x - x` is 0, and two totals that both read the fuel price move together. A design where `+` combined two distributions as if they were unrelated would get both wrong, which is the mistake [DESIGN.md](../DESIGN.md) records for robustness as a number.
- **A number is a number.** `number` is still the type. Whether a number is uncertain is like whether it differs between worlds: not part of its type, and nothing a formula's author has to think about.

## Writing one

A distribution is written as a formula, so nothing new is saved in a file:

```
Rate:      normal(0.05, 0.01)
Estimate:  triangular(2, 3, 6)
Die:       integers(1, 6)
Late:      bernoulli(0.2)
```

| Function | Means |
|---|---|
| `normal(mean, sd)` | A bell curve |
| `lognormal(mean, sd)` | A number whose log is `normal(mean, sd)`, so it's never below 0 |
| `uniform(low, high)` | Anywhere between two numbers, equally |
| `triangular(low, likely, high)` | A three-point estimate |
| `exponential(mean)` | A wait with that average |
| `between(low, high)` | A normal with 90% of its weight between the two: "about 2 to 4" |
| `integers(low, high)` | Each whole number from one to the other, equally |
| `bernoulli(p)` | True with chance `p` |

The arguments are ordinary formulas, so they can read other cells, differ between worlds, and be uncertain themselves: `normal(base.value, 0.1 * base.value)`.

**Each place one is written is its own quantity.** A cell that says `normal(0.05, 0.01)` is one uncertain rate, and everything that reads the cell reads the same rate. Written twice, as `normal(0, 1) + normal(0, 1)`, it's two. A formula that runs for each row of a table, or each node of a kind, or each node a fold goes over, is written once for each, so `growth = normal(0.05, 0.01)` as a column is a draw per year, while every year reading one `Rate` cell is one draw shared.

**A choice over distributions needs nothing of its own.** `car.chosen.cost` is whichever car's cost, world by world, and each can be held differently: one a plain number, one a normal, one a tree.

## How a number is held

A number is a plain number, or a handle to an expression over **draws**. Expressions live in one table shared by every value, and the same expression always gets the same handle, so comparing two is comparing handles, and a memoised cell stops what reads it from rerunning when its expression hasn't changed, as a `Bool` does.

- **A draw is a standard one, with no parameters,** named for where it's written: the node, the cell, and which draw of that formula it is. `normal(mu, sigma)` is an expression over it, so its parameters can be anything a number can. Changing sigma keeps the draw, so what's shown moves smoothly between edits, and two what-ifs are compared on the same draws.
- **A distribution over a short list is a variable in the diagram,** the same as an `uncertain` decision: its outcomes and their weights. `integers(1, 6)` is one, so two dice added are eleven cases with exact chances, by the code that already does that.
- **Everything else is a tree,** exact as a function, however it's then read.

Each expression is held in the simplest exact form there is:

| Form | Example | Read |
|---|---|---|
| A plain number, or cases | `3`, two dice | Exactly, as today |
| A named law | `normal(3, 0.5) * 2 + 1`, a sum of unrelated normals | Exactly: its mean, spread and any point of it |
| A tree | `max(a, b) * rate` | By drawing, saying so |

**Laws are only combined when what they depend on is separate.** A sum of two normals is a normal only if they share no draw. That check is what makes simplifying safe, and a rule can be added one at a time, because the tree is right without it.

**A histogram is for reading, never for working out.** One number's histogram can't answer `x - x`. If a tree is ever too big to keep, what stands in for it is a column of draws lined up with every other column, which still can. A histogram of a person's own data can be a distribution to draw from.

## Truth under uncertainty

`x > 5` is true in some draws and false in others: a distribution over the two. It's held as numbers are.

- **With nothing uncertain behind it but choices and short lists, it's the diagram,** exactly as before.
- **Otherwise it's a tree,** combined by `&&`, `||` and `!` with the same three values, so `false && undefined` is still false, draw by draw.
- **"Never" and "always" are said only when they're shown.** `x > 5 && x < 3` simplifies to never. Where simplifying can't settle it, what's said is a chance, with how it was got.

## What's shown, and how it says it was got

Every value shown came about one of three ways, and says which:

| How | Shown as |
|---|---|
| Exactly: counted, a named law, or simplified to an answer | The value |
| By drawing | The value, marked as an estimate, with how many draws and how far out it could be. Everywhere: a card, the table, and what Claude reads |
| It can't be worked out correctly | An error in the cell saying why. Never a guess |

- **Drawing is repeatable.** A draw's values come from its name, so the same graph gives the same picture, a what-if and the real thing use the same draws, and a memo is still a memo.
- **A truth that was false in every draw reads "0 in 4,096",** not "never", and nothing is dimmed or ruled out because of it.
- **How likely counts as possible is the view's to say.** A normal estimate is over 5 days in some draw, always, so a notice like `oversized_task` asks the view for a level and says which it used. What refuses a commit can't depend on a view, so it stays with what's shown always or never.

A cell with a distribution shows its histogram, its middle and how wide it is. One whose value also differs over a choice shows one for each answer, and over something uncertain, the mixture.

## What's known, and what's demanded

`requires` drops the worlds where its value is false, and that has been read two ways that happened to agree:

| Reading | Example | With something uncertain |
|---|---|---|
| **This is known** | "The thief arrived by ten" | Drop the draws where it's false, so beliefs shift. Nothing of yours is ruled out |
| **This is demanded of my choices** | "It has to cost under £4,000" | Rule out the options that don't meet it. How the cost is distributed is untouched |

With only choices, a world is an option, so both drop the same thing. With a distribution they part: read as known, a budget keeps every option and cuts the cost off at £4,000, as if going over couldn't happen. So they're two components:

- **`evidence`:** this is known to be true. It conditions what's uncertain, and never restricts a choice of yours.
- **`requires`:** your choices have to make this true, with a cell for how surely, which is always unless it says. For each way your choices could go, its chance, given the evidence, has to reach that.

With nothing uncertain, each does exactly what `requires` does now.

**A choice is judged by what's known when it's made.** A `knowing` edge goes from a decision to each uncertain thing that's known by then. A requirement is then judged for each thing that could be known: an option is allowed where its chance of meeting it, given that, reaches the level. That's a value that differs over the known things, so pinning what Monty opens shows which options are left. Choices in a row are worked backward, each later one made as well as it can be. With no `knowing` edge, a choice is made before anything's known, which is what the graph says, and its card says so by name: "judged as chosen before X is known".

A choice that knows an uncertain number, as "decide once the quote's in", has no exact answer here yet, so it's an error in the cell that says so.

## Pinning

- **To a value:** "what if the rate's 6%" puts 0.06 where the draw was. It's the same as typing it in, so a what-if and the real thing can't disagree.
- **To a range:** "it's under 10" is something found out, the same as evidence.

A sweep is pinning to each of a list of values, with no copy of the graph.

## Collapsing, in a cell

`expected(x)` has been kept out of cells, because one number can't be narrowed. Held as a part of a tree, worked out only once a view has narrowed it, it can be: `chance(cost < 4000)` and `mean(cost)` average over what's uncertain and never over a choice of yours, so each is still a value per way your choices could go. That's what a requirement's level is underneath.

## What changes where

| Where | What |
|---|---|
| `recollect-graph/src/chance.rs` | The table of expressions, draws, the families, simplifying, laws, and drawing |
| `worlds.rs` | A pin can be a value or a range. A chance can come with how it was got |
| `expr.rs`, `formulas.rs` | A number can be an expression. The families, and `exp`, `ln` and `sqrt`. Each draw written gets its own name, which is why identical parts of a formula can no longer always be worked out once |
| `plan.rs`, `schedule.rs` | An estimate and a date can be uncertain. The dates are formulas already, so they carry it |
| `recollect-core/src/views.rs` | A cell's distribution: its law if it has one, its middle and width, a histogram, and how it was got |
| `web/` | A histogram wherever a number is shown, a fan down a table's rows, and a slider to pin with |

Files don't change: a distribution is a formula, and only its text is saved.

## Limits

- **What's read is numeric unless there's a law.** The tree is exact, and a chance read from it by drawing is an estimate that says so.
- **A list too long to count is drawn.** `integers(1, 1000000)` isn't a million cases.
- **Counting worlds counts choices and short lists.** There's no number of draws.
- **`x = 5` on a number that could be anything has no chance of being true,** and is worth a notice.
- **The table of expressions is never cleared,** as the diagram isn't.
- **A loop is still reported if any world has it,** and a draw doesn't change which cells read which.

## What it opens up

- **The cap on distinct values mostly stops mattering for what's uncertain.** Forty uncertain estimates added are one expression, where forty priced choices are 2⁴⁰ cases.
- **A schedule's dates are right about merging.** The later of two uncertain finishes is later, on average, than either, and working that out draw by draw gets it, where adding averages doesn't. How often each task is the one holding the end up comes with it.
- **Options are compared fairly.** How often A beats B is counted on the same draws, so a fuel price both depend on cancels.
- **What an answer is most sensitive to** is which draw its spread follows, read from columns that are already there. It's the same question as what's most worth finding out.
- **"About three weeks" can be written down** by Claude as `between(2, 4)`, and reviewed like any formula.

## Order of building

| Step | What | State |
|---|---|---|
| 1 | Trees and drawing: the families, arithmetic, comparisons and `if` over them, short lists as diagram variables, and a histogram wherever a number is shown, saying how it was got. A requirement on something uncertain is refused, with why | Built |
| 2 | Laws: named ones, the rules that keep them, and a chance read exactly from one | Built: the rules are below, and more can be added one at a time |
| 3 | `evidence` apart from `requires`, with a level, and conditioning on what's uncertain | Built, but for evidence that's a matter of draws in some worlds and not others, which is refused |
| 4 | Pinning a quantity to a value or a range | Built |
| 5 | `knowing`, and choices worked backward | Built, for choices that all know the same things, and things with short lists of outcomes |
| 6 | `chance` and `mean` in a cell | Built |

## What's built

**Simplifying.** Each of these changes nothing in any draw, which the core property checks on random formulas:

- plain arithmetic is done there and then, and adding nothing, multiplying by one, and a number less or over itself go;
- `max` and `min` go where one side can't be the other;
- a comparison is settled where the two sides' limits don't meet;
- two limits on one number with nothing between them are never, and two that leave nothing out are always, as are something and its opposite;
- not below is at or above, and a choice on the opposite, or on at-or-below, is the same choice the other way round, so each is held one way.

Most of these need both sides to be defined in every draw, and are left alone where they might not be.

**Laws.** What a number is exactly, where that can be said:

| What | Is |
|---|---|
| Any sum of bell curves, each scaled and shifted however, whatever they share | A `normal` |
| A `uniform`, an `exponential` or a `triangular`, scaled and shifted | Still one |
| A product, quotient, power or root of things whose logs are bell curves, whatever they share | A `lognormal`: ten years of `exp(normal(…))` growth is one |
| The log of one of those | A `normal` again |
| The later, or the earlier, of things that share no draw, each with a law or a plain number | A law of its own, written `max(…)` or `min(…)`. With a plain number among them, some of its weight is on that number: `max(x, 0)` is exactly 0 half the time |
| Any of those moved or stretched by a plain number, where that has no name | Itself, moved: `10 - exponential(2)` |
| Each of several laws or plain numbers some of the time | Their mixture, which is what a cell that differs over chance comes to taken together |

A law says how much of its weight is below any point, and everything else is read from that. One with a name has formulas for its mean and spread. The later-of and earlier-of have none, so theirs are summed between a few hundred points, and are right to about nine figures.

**A chance from a law.** A truth is read exactly where a law answers it:

- one number against another, where the gap between them has a law, as `a < b` for two bell curves does, or where each is a product whose log is a bell curve;
- a number with a law against a plain one, including whether it's exactly that number: `max(x, 0) = 0`;
- two limits on one such number, between them or outside them;
- two such questions about things that share no draw, both or either;
- and the opposite of any of those.

Anything else is read from 4,096 draws. Each draw's values are spread evenly, one to each 4,096th, in an order that comes from its name.

**What a view sends.** Wherever an uncertain number would be, there's `{"dist": …}`: its mean, spread, the points it's below one time in twenty, half the time and nineteen in twenty, a histogram, its law if it has one, and `exact`, with `draws` and `error` when it isn't. An uncertain truth is `{"chance": …}`, with how many draws it held in. A cell that differs only over what's a matter of chance has a `mix` as well: what it comes to taken together. Over a choice of yours there's no `mix`, because averaging over that means nothing.

**What's known and what's demanded.** `evidence` and `requires` are two components, as above. `requires` has a cell, `at_least`, for how surely.

- **A requirement is judged for each way the choices could go.** A choice is whatever isn't a matter of chance: a decision that isn't `uncertain`, or a puzzle's unknown. For each, how likely the value is to be true is summed over every matter of chance, given the evidence, and held against `at_least`. With no matter of chance that's just where it's true, which is what `requires` always was.
- **Evidence about an uncertain number leaves only the draws where it holds.** What shares a draw with it is read from those, and says how many they were. What shares none is untouched, and keeps its law.
- **Pinning something that's a matter of chance is finding it out,** so a requirement is judged again knowing it.
- **How it was judged is a notice.** A requirement that depends on chance says what it's judged as chosen before. One that no choice can make sure enough says so, and that it may be evidence. One whose chance was read from draws and came too close to its level to call says that.

**Pinning.** A view looks through a copy of the graph with what's pinned written into it, so a what-if is doing it for real, on a copy:

- **A quantity pinned to a value** is typed in. It's in a world as `=node/cell` with the number.
- **A quantity pinned to a range,** `3..10`, `..10` or `3..`, is a piece of evidence that it's in that range. One that's uncertain in some worlds and not in others, or that no draw fits, is refused and says why.
- **A matter of chance pinned to an answer** is found out: `chance` and `mean` read it, as they'd read it settled.
- What the view adds to look through is never shown, and never saved.

**`chance(x)` and `mean(x)`** average over every matter of chance, given what's known and what's been found out, and never over a choice of yours: there's one for each way your choices could go. A `requires` can then say `chance(cost < 4000) >= 0.9`.

**What's read from draws says so wherever it goes.** Where `mean` or `chance` has no law, the cell holds an estimate: what it came to, from how many draws, and how far out it could be. Adding, taking away and scaling by a plain number keep how far out, which is never less than it is. Anything else keeps that it's an estimate, and no longer how far out. A comparison is true or false only where the two are further apart than three times that, and otherwise undefined, since it can't be called. A cell that has to hold a number, such as a task's estimate, can't hold one.

**`knowing`** is an edge from a decision to something it's made knowing the outcome of. A requirement is then judged for each way that could turn out: an option is allowed where its chance, given that, is enough. A choice made before is allowed only where, however that turns out among the ways it can, the later choice has something allowed. In Monty Hall, requiring a win at least 0.6 of the time leaves only switching, whichever door he opens.

**What it leaves out**, beyond the steps above:

- **Choices that know different things,** in a row, aren't worked out. A requirement like that rules nothing out, and a notice says why.
- **A choice that knows an uncertain number** isn't worked out either, in the same way.
- **`chance` and `mean` don't condition on what a choice knows.** They're over everything that's a matter of chance.
- **An estimate can't be combined with something uncertain,** which would need the same draws: that's an error in the cell.

- **The level a notice uses is fixed,** where a view should say it. `oversized_task` fires when an estimate is more likely than not to be over five days, and says how likely, and whether that's exact or from draws.
- **A truth is only ever shown always or never by simplifying.** `x > 1 || x <= 1` simplifies to always. `abs(x) >= 0` doesn't, so it's true in every draw and reads "all 4,096 draws", not "always".
- **An uncertain condition can only choose between numbers, or between truths.** Choosing between text or nodes is an error in the cell.
- **`done`, `dropped` and a weight can't be uncertain.** Each is an error that says so.
- **Evidence that's an uncertain truth in some worlds and not in others** is refused. So is nothing else about evidence: an uncertain truth that's the same in every world conditions the draws.
- **The big-task notice doesn't know what's known.** It reads an estimate as it is, not given the evidence.
- **A sum of unlike things has no law.** Two `triangular` estimates added, or a `triangular` and a `uniform`, are read from draws. So are a later-of plus anything uncertain, and anything through an `if` on an uncertain condition.
- **A law is only looked for so far down.** Past 48 things each made of the last it's read from draws, which is always right.
- **A person's own data as a distribution, a fan down a table's rows, a slider to pin with, and a sweep by putting a value in** aren't built.
- **A tree is never swapped for columns,** however big it gets, and a formula's size is still capped as it was.

## How it's checked

| Check | How |
|---|---|
| **The core property** | Pick a world and a value for every draw. The tree worked out there is exactly what a plain evaluator gives on ordinary numbers |
| **Simplifying changes nothing** | Each rule, on random expressions: the same value at every draw tried |
| **A law is the tree's** | A law's mean, spread and points against the tree's own draws, within what drawing allows |
| **Without anything uncertain, nothing changed** | Every test and sample from before |
| **Drawing is repeatable** | The same graph, built twice, gives the same histogram |
| **The lens is honest** | Pinning a quantity then reading equals typing it in through a real commit, then reading |

## The samples

| Sample | Shows |
|---|---|
| **A launch, give or take** | A plan whose estimates are three-point guesses: what they add up to, when it ends, and how likely that is to be within a fortnight. One thing in it is a coin toss, so what depends on it shows both answers and what they come to together |
| **Savings, give or take** | The same formula down two tables. Growth drawn afresh each year averages out, and one rate drawn once compounds. Both ends are lognormals, exactly |
| **Choosing a supplier** | A choice over quotes held three ways: a plain number, a three-point guess, and a price with extras. One is exact and two are read from draws |
| **Will the shelf fit?** | Two measurements subtracted, which is a bell curve, and questions a law answers. A cell less itself is exactly nothing |
| **Two dice** | Short lists: eleven totals with exact chances, and what finding out one die says about the rest |

`tests/samples.rs` checks each says what it claims.
