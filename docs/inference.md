# Inference: what's seen, and how likely it was

**Status:** a design. Nothing here is built. It follows from [data.md](data.md#fitting), where a straight line through forty runners, written as equations, can only be told about the data with a truth, and runs out of draws. It extends [What's known, and what's demanded](distributions.md#whats-known-and-whats-demanded).

## Why

`evidence` says something is so, and keeps the worlds and draws where it is. That's right for "the thermometer reads 40, to the nearest degree", and wrong for "this runner's 10k was 52.0 minutes", which no draw ever exactly is. What's wanted is how likely each way things could be makes what was seen: a weight, not a truth. A line that misses a runner by a little is likelier than one that misses by a lot, and neither is ruled out.

Two things are missing, and this is both:

1. **A way to write what was seen and how it's spread:** an observation.
2. **A way to read with it** that doesn't run out of draws.

## One component, not three

`evidence` and `requires` were split because one mechanism was being read two ways ([distributions.md](distributions.md#whats-known-and-whats-demanded)). An observation could be a third component. It shouldn't be:

- **An observation is what evidence already is, in general.** "This is so" is the observation whose likelihood is 1 where it's true and 0 where it isn't. Conditioning on a truth is weighting by that. So `evidence` means "this was seen", and what was seen is either a truth or an observation. Everything evidence does now is the special case, unchanged.
- **The form says which, so nothing is overloaded.** A truth is a formula that's true or false. An observation is written with `~`, below. The engine never decides which from what the value happens to be, and the type checker knows before any world is looked at.
- **`requires` stays a truth.** It's a demand on your choices, judged for each way they could go, and a demand is met or it isn't. How surely is already its `at_least`. A weighted `requires` would be a preference, "better if cheaper", which is choosing by what's best rather than what's allowed. That's worth a design of its own, and isn't this.

## Writing an observation: `~`

```
r.t10k ~ normal(m * r.t5k + c, sigma)
```

reads "r.t10k is drawn from normal(m·t5k + c, σ)", which is how a model is usually written down.

- **On the left is what was seen:** any number, usually a plain one from data. It can be uncertain: the weight is then how likely the right side makes each draw's value.
- **On the right is a distribution, written as one is anywhere else.** There it isn't a draw: it's the law the left is said to come from. Its arguments are ordinary formulas, so they can read other cells, be uncertain, and differ between worlds, which is the point: `m` and `c` are what's being found out.
- **Its value is how likely,** for each world and draw: the right side's density at the left, or its probability for a short list. That's a new kind of value: a statement, as a truth that's evidence is one. It can be in any cell, and it conditions anything only once it's evidence. Until then it's something that could be seen and isn't known yet, as the tea's second reading is, and its cell says how surprising it would be.
- **`&&` is both seen,** so it multiplies. An observation and a truth together multiply by 1 or 0. `all(r in T.rows: …)` is every row seen. `||`, `!` and `if` over an observation are errors: "one or the other was seen" isn't something anyone sees.
- **Each family has how likely it makes a value.** `normal`, `lognormal`, `between`, `uniform`, `exponential` and `triangular` have densities. `bernoulli`, `integers` and `one_of` have probabilities, and the left side of one of those is a truth, a whole number, or a node.

### On a table

A table's own `value` is its own and not its rows' ([components.md](components.md)), which is how a puzzle says `distinct(rows)` about all its rows at once. A statement about every row is written the same way, as the table's own value, and the table is evidence:

```
Runners:  all(r in rows: r.t10k ~ normal(m * r.t5k + c, sigma))      evidence
```

Leaving one runner out to see how much they move the answer would be a row's own cell read by that formula. That's a convenience for later, and needs nothing new.

## Saying what's what: parameters, results, verbs

Spreadsheet formulas are definitions: this cell is that. Evidence and requirements are statements: this was seen, this has to hold. They read oddly as formulas, and neither kind says much about what goes in or comes out. Ideas, from most worth doing to least:

1. **`~`, shown as words.** It's typed `~` and, wherever a statement is shown (a card, the Inspector, what Claude reads), it reads "drawn from": *Known: t10k drawn from normal(mean m·t5k + c, sd σ), for each runner*.
2. **Named arguments for anything built in:** `normal(mean: m * x + c, sd: sigma)`, `triangular(low: 2, likely: 3, high: 6)`. Positional still works. A name is checked against the function's parameters, and the formula's text is kept as written. It says in the formula what each number is, which is most of what's hard to read in one.
3. **The editor says what a function takes and gives as you type:** `normal(mean, sd)`, *a number drawn from a bell curve*, and for any cell, what kind of thing it holds: a number, a truth, a number that's uncertain, or a statement.
4. **A statement is shown as one.** A node that's evidence or a requirement shows its statement, with no `=`, under *Known* or *Required*, and the field that edits it is called that rather than *Value*.
5. **A few words as operators, where they read as a verb.** `now within 0.5 of 40` for `abs(now - 40) < 0.5`. Each is one more thing to learn, so only where a symbol can't say it: `~` is the one that's needed now.

The first two are worth doing with `~`, and the rest when they're needed.

## Reading with weights

What a reader is given changes from a truth, which keeps or drops each draw, to a weight for each draw, held as a log so forty densities multiplied don't vanish. A truth gives 1 or 0, so every reading that's right now stays exactly the same.

- **What's read says how many draws it's worth.** Weights spread over 4,096 draws are worth `(Σw)² / Σw²` of them, all the same: the effective draws. Every value read under weights says so, "≈ 2.10 ± 0.02, from 22 effective draws", and its error is from that, never from 4,096.
- **Too few to say anything is a notice,** at a level the view says, as for oversized tasks.
- **Where only short lists and choices are involved, it's exact.** An observation of a die's roll, `roll ~ integers(1, die.chosen.sides)`, multiplies each world's weight by its probability, as an option's weight does now. Which die it was is then exact, as it is with a truth.
- **Where worlds and draws are both involved,** a world's weight is multiplied by how likely it makes what's seen, averaged over its draws. That average is read from draws, so beliefs about the worlds become estimates, and say so. (Today evidence that's a matter of draws in some worlds and not others is refused. That can stay refused until this is built.)
- **`chance`, `mean` and requirements** are read with the weights, and "too close to call" uses the effective draws.

That's enough for a few rows, or wide observations, and it's honest about the rest. With forty runners it's 22 draws, which is why the next part is needed.

## Drawing that follows the weights

Weighting the prior's draws can't do better than the prior's draws. The answer is to move the draws to where the weight is.

- **Everything uncertain is already a function of named standard draws,** each a number from 0 to 1. So moving them needs to know nothing about the model: move only the draws the evidence depends on, and leave the rest alone, since what shares no draw with the evidence isn't changed by it.
- **The 4,096 draws become 4,096 particles.** Starting from the draws read now, the weight is brought in a little at a time, raised to a power from 0 up to 1, in steps sized so the effective draws never fall below half. After each step the particles are resampled, and each is moved a few times, a step at a time, in the space of its bell-curve scores, accepting a move as often as the weight says it should (Metropolis), with step sizes from the particles' own spread. This is sequential Monte Carlo with tempering.
- **Each move is one pass over a column,** which is how everything uncertain is already read: the weight's expression, worked out for all 4,096 at once.
- **Why this way, and not one long chain:** it starts from what's there, keeps several humps where there are several (the tea has six), says how many draws it's worth at every step, and gives, as it goes, how likely what was seen was, which is what weighs the worlds against each other.
- **It's repeatable.** Every choice it makes comes from the draws' names, so the same graph gives the same answer, and a what-if is read with the same moves.
- **Afterwards, everything tied to the moved draws is read from where they ended up,** still lined up draw by draw, so `x - x` is still 0 and two things that share a draw still move together. A law isn't used for anything tied to evidence, as now.
- **What it costs, and what it says.** The line through forty runners is an expression of about 500 parts, so a pass is about two million small sums, and twenty steps of five moves is a fraction of a second, natively. There's a budget: past it, what's read is what there is, with how many draws it's worth. And it says how it was got: *drawn, then moved to fit what's known, worth 3,100 draws*, with a warning where moves were rarely accepted, which is when this can be wrong without the numbers showing it.

## Exact where it can be

Some fits need no drawing at all, as sums of bell curves need none now. When the unknowns are bell curves, each observation is a bell curve whose middle is a straight line in them, and its spread is plain, what's known about them afterwards is a bell curve over them too. Recognising that is a rule like the ones laws already have: then the line through the runners, with σ known, is exact. With σ uncertain too it isn't, and it's drawn. This is a rule that makes some answers exact, and changes nothing about how a model is written.

## The runners, as they'd be written

```
m:            normal(mean: 2, sd: 0.5)
c:            normal(mean: 0, sd: 5)
sigma:        exponential(mean: 2)
Runners:      all(r in rows: r.t10k ~ normal(mean: m * r.t5k + c, sd: sigma))      evidence
A 22-minute 5k runner's 10k: normal(mean: m * 22 + c, sd: sigma)
```

Everything `predict` did is here, in the open. The prior is two cells anyone can change, the spread is a third, the noise is a bell curve because it says so, and a prediction is an ordinary formula that reads the same `m`, `c` and `sigma` as everything else, so two predictions are related because they share them, with no special naming. A curve, a log scale or a difference between groups is a change to one formula.

## What changes for what's there

- **Evidence that's a truth means what it did.** Every test and sample from before still holds.
- **The tea** can say what its thermometer does: `40 ~ normal(mean: now, sd: 0.3)` in place of `round(now) = 40`.
- **The box** of [data.md](data.md#what-the-engine-makes-of-it) still works and still means a box.

## Order of building

| Step | What |
|---|---|
| 1 | `~` and the families' densities, as evidence, read with weights and effective draws, and exact over short lists |
| 2 | Named arguments, and the editor saying what a function takes and gives, and a statement shown as one |
| 3 | Moving the draws: tempering, resampling and moves, with its budget and what it says |
| 4 | Bell curves seen through straight lines, exactly |

## How it'd be checked

| Check | How |
|---|---|
| **A truth as evidence is as it was** | Every test and sample from before |
| **An observation over short lists is Bayes' rule** | Which die it was, from rolls, against the sums done by hand |
| **Weighted reading is honest** | Effective draws against `(Σw)²/Σw²`, and a few rows of the line against a grid |
| **Moving gets there** | The line through forty runners against a grid, 2.099 ± 0.043 with σ = 1. The tea's six humps, each the right size. Two humps far apart, both kept |
| **It's repeatable** | The same graph, built twice, the same answer |
| **Exact is exact** | Bell curves seen through straight lines, exactly, against moving draws, within what drawing allows |
