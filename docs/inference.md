# Inference: what's seen, and how likely it was

**Status:** steps 1 and 3 are built: `~`, evidence that's a weight, exact over short lists, read with weights over draws where that's enough, and with the draws moved to fit it where it isn't, each saying how it was got. [Order of building](#order-of-building) says what's left, and [What step 1 is](#what-step-1-is) and [What step 3 is](#what-step-3-is) how they're done and what they leave out. It follows from [data.md](data.md#fitting), where a straight line through forty runners, written as equations, can only be told about the data with a truth, and runs out of draws. It extends [What's known, and what's demanded](distributions.md#whats-known-and-whats-demanded).

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

- **What's read says how far out drawing could put it, in its own units.** Weights spread over the draws count as `(Σw)² / Σw²` of them, the effective draws, and a reading's error is from that, never from how many there are. That error is what's said: "≈ 2.10 ± 0.04, drawing could put the mean out by about 0.001". How many draws there are is a budget, not something about what's read, so it's never shown as part of a value; it's for diagnosis, in notices.
- **Too few to say anything is a notice,** at a level the view says, as for oversized tasks.
- **Where only short lists and choices are involved, it's exact.** An observation of a die's roll, `roll ~ integers(1, die.chosen.sides)`, multiplies each world's weight by its probability, as an option's weight does now. Which die it was is then exact, as it is with a truth.
- **Where worlds and draws are both involved,** a world's weight is multiplied by how likely it makes what's seen, averaged over its draws. That average is read from draws, so beliefs about the worlds become estimates, and say so. (Today evidence that's a matter of draws in some worlds and not others is refused. That can stay refused until this is built.)
- **`chance`, `mean` and requirements** are read with the weights, and "too close to call" uses the effective draws.

That's enough for a few rows, or wide observations, and it's honest about the rest. With forty runners it's about 20 draws, which is why the next part is needed.

## Drawing that follows the weights

Weighting the prior's draws can't do better than the prior's draws. The answer is to move the draws to where the weight is.

- **Everything uncertain is already a function of named standard draws,** each a number from 0 to 1. So moving them needs to know nothing about the model: move only the draws the evidence depends on, and leave the rest alone, since what shares no draw with the evidence isn't changed by it.
- **The 4,096 draws become 4,096 particles.** Starting from the draws read now, the weight is brought in a little at a time, raised to a power from 0 up to 1, in steps sized so the effective draws never fall below half. After each step the particles are resampled, and each is moved a few times, a step at a time, in the space of its bell-curve scores, accepting a move as often as the weight says it should (Metropolis), with step sizes from the particles' own spread. This is sequential Monte Carlo with tempering.
- **Each move is one pass over a column,** which is how everything uncertain is already read: the weight's expression, worked out for all 4,096 at once.
- **Why this way, and not one long chain:** it starts from what's there, keeps several humps where there are several (the tea has six), says how much of the drawing counts at every step, and gives, as it goes, how likely what was seen was, which is what weighs the worlds against each other.
- **It's repeatable.** Every choice it makes comes from the draws' names, so the same graph gives the same answer, and a what-if is read with the same moves.
- **Afterwards, everything tied to the moved draws is read from where they ended up,** still lined up draw by draw, so `x - x` is still 0 and two things that share a draw still move together. A law isn't used for anything tied to evidence, as now.
- **What it costs, and what it says.** The line through forty runners is an expression of about 500 parts, so a pass is about two million small sums, and twenty steps of five moves is a fraction of a second, natively. There's a budget: past it, what's read is what there is, with how far out that could put it. And it says how it was got: *read from draws moved to fit what's been seen*, with a warning where moves were rarely accepted, which is when this can be wrong without the numbers showing it.

## Exact where it can be

Some fits need no drawing at all, as sums of bell curves need none now. The idea that makes that general is that **what's exactly so, given what's been seen, is a change of the draws, not a new kind of reading.**

Every uncertain number is an expression over named standard draws, and the reader already keeps two forms of each: a straight line over draws, and a straight line for its log. A straight line over bell-curve draws, each once, has an exact law, and that's how sums, scalings, products of lognormals and chances like *slope > 2* are exact now, through any formula. So if what's been seen can be taken in by replacing some draws with what they exactly are given it, as straight lines over new draws, everything read through them keeps its law with nothing else new. The laws are the identities that carry exactness through formulas; taking in what's seen only has to hand them new draws.

- **Recognising it.** Beside the two forms there's a third: a quadratic over draws. A straight line squared is one, and sums and scalings of them are. `y ~ normal(mean, sd)` has the log density `-(y - mean)²/2sd²`, which is one exactly when `mean` is a straight line over draws and `sd` is plain. The line through the runners is: `slope * r.t5k + height` is straight in the slope's and height's draws, since `t5k` is data. So recognising it falls out of the forms there already are, with nothing matched against how a formula is written.
- **The substitution.** With standard bell-curve draws `z` beforehand, and what's seen weighing them by `b·z + zᵀAz`, they're a bell curve afterwards, exactly: precision `P = I - 2A`, middle `P⁻¹b`. Each `z` is replaced by its middle plus `M·ε`, where `ε` are new standard draws and `M·Mᵀ = P⁻¹`. Every formula that read `z` is now a straight line over `ε`, so the slope is `normal(2.0989…, 0.0433…)` exactly, the slope and height are related as they should be because they share `ε`, and the next runner's 10k and how likely it is to be under 46 are exact too.
- **What's left.** What's seen is a sum of logs, and only the parts that are quadratics over bell-curve draws are taken in this way. The rest, and any truth that's known, weigh or move the draws as before, over what's exact. That's still right, since weighing what's exactly so given some of what's seen by the rest is exactly Bayes' rule, and it's how an uncertain σ, or a middle that's not a straight line, goes on being drawn.
- **The same, beyond bell curves.** Every draw is a number between 0 and 1 put through a law's quantile, so anything whose law given what's seen has a quantile can be a new draw put through it. That makes exactness a table of rules, each a prior's form and a likelihood's form going to a law: a uniform chance and counts going to a Beta, an exponential rate and waits going to a Gamma, and anything seen about a single draw, such as a truth that cuts a bell curve off, whose law is one-dimensional and can be worked out to as many figures as a number holds. Each rule is small, tested on its own, and never touches how anything is read.
- **Partly exact.** Where what's seen is a bell curve about a line through `z` only once some other draws `w` are fixed, as when σ is uncertain, each draw of `w` gives `z` a bell curve, and how likely what's seen is given `w` alone has a closed form. Weighing or moving only `w` by that, and replacing `z` draw by draw, needs far fewer dimensions moved. This is what other probabilistic languages call delayed sampling, or automatic Rao-Blackwellisation (Murray et al., 2018).
- **Comparing models.** The same algebra gives exactly how likely what was seen was, all told. Where a model differs between worlds, a line against a flat one, say, that's the chance of the coin in each, so which model is likelier is exact Bayes through the diagram.
- **What it says.** What's read this way is `given: exact`, and `exact` where it has a law. A notice on the evidence says how many draws were replaced, and, where some of what's seen wasn't taken in exactly, that the draws are weighed or moved by the rest.

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
| 1 | `~` and the families' densities, as evidence, read with weights and effective draws, and exact over short lists. **Built** |
| 2 | Named arguments, and the editor saying what a function takes and gives, and a statement shown as one |
| 3 | Moving the draws: tempering, resampling and moves, with its budget and what it says. **Built** |
| 4 | Bell curves seen through straight lines, exactly. **Built**, as the first rule of [exact where it can be](#exact-where-it-can-be) |
| 5 | Exactly how likely what's seen was, for comparing models between worlds |
| 6 | Partly exact: what's a bell curve about a line once other draws are fixed |
| 7 | More rules: a Beta for a chance seen through counts, a Gamma for a rate seen through waits, and anything seen about one draw |

## What step 1 is

- **`~` is an operator at the level of a comparison,** so `a ~ normal(m, 1) && b ~ normal(m, 1)` needs no brackets. The right is checked when it's read: it has to be one of the families. A statement's type is *something seen*, and the type checker allows `&&` with it and nothing else that combines truths: `||`, `!`, `if` and comparing one are errors that say why. A requirement that's something seen is refused when it's written.
- **How likely each family makes a value is written as arithmetic** on the same expressions everything else is, in `log_likely` in `formulas.rs`, so it's the same whether what goes in is plain or uncertain, and where what was seen couldn't have come from it, or it isn't a distribution, as with a spread below nothing, it's undefined, and weighs nothing.
- **Over short lists, it's a chance of its own.** Where how likely it was is the same in every draw, what's seen is a coin whose chance of heads in each world is how likely it was there, which is `bernoulli` with a chance that differs between worlds. Evidence keeps the heads, so the worlds are weighed by Bayes' rule by the diagram, exactly, and every belief, requirement and view that reads the worlds is right with nothing new. For a density the chances are each relative to the likeliest world, which changes nothing evidence does. The coin is bookkeeping, so it's called for where it's written with `seen` after it, and isn't shown as a question.
- **Over draws, a reader weighs them.** `Known` is a truth and the log of how likely what was seen is. A truth gives each draw 1 or 0, as before, so nothing that was right changed. What's read under weights is `weighted`, and its `draws` are how many count, `(Σw)²/Σw²`, which its error is worked out from. A mean or a chance carries that into the cell, and the app says *read from draws weighted by how likely each makes what's been seen, so drawing could put the mean out by about 0.02*.
- **Something seen that isn't known yet** is shown as how much it would say if it were: how many-fold it narrows the worlds, `{"seen": {"narrows": 240, "at_least": false}}`, *seen: narrows the worlds about 240-fold*. That's how many draws there are for each that weighing by it alone leaves counting, which is 1 plus its χ² divergence from what was believed before, so it doesn't depend on how many draws there are. Read from fewer than 100 that count, it's likely more than that, since what's rare enough to be missed is what it counts most, so it says *at least*; from none, *nothing drawn fits it*. It's weighing alone, never moving, since it's how much the statement says on its own.

It leaves out, beyond the later steps:

- **Evidence that's a matter of draws in some worlds and not others** is still refused, weights or not. So is something seen whose likelihood depends on draws and differs between worlds.
- **A statement's own chance, shown in its cell, isn't a likelihood.** Where it's a coin, a cell that reads one shows it as true or false, as `bernoulli` would. Only evidence gives it a meaning.

## What step 3 is

- **When:** where weighing the draws by what's been seen would leave less than half of them counting. Otherwise they're weighed, as in step 1, which is cheaper and as good. A truth alone never moves anything: what's known only as a truth keeps or drops draws, exactly as before.
- **What moves:** the standard draws the evidence depends on, and nothing else. Anything that shares none of them is read as it was, law and all.
- **How:** in `move_draws`, in `chance.rs`. Each draw's place is held as its bell-curve score. Whatever's impossible goes first. Then the weight is brought in a step at a time, each step as far as leaves half the draws counting; after each, the draws are kept as often as they weigh (systematic resampling) and moved, all 4,096 at once, by Metropolis steps scaled by their own spread, until each has moved about three times. A step that's taken too often gets longer, and too rarely, shorter. At most 200 steps and 20 moves after each.
- **Every choice it makes** comes from a stream seeded with the moved draws' names, so the same graph gives the same answer.
- **What it says:** what's read is `given: moved`, and its `draws` are what the last step's weight was worth before it was resampled, never more than how many are distinct, since moved draws are related to where they came from. The app says *read from draws moved to fit what's been seen*, and how far out drawing could put it, from that. A notice on the evidence says how many steps, how many are distinct, what share of the draws they're taken to count as, and how often the last moves were taken, and warns when that's under one in ten.
- **What it costs:** the line through forty runners is six steps and about 0.2 seconds natively.

It leaves out:

- **How related the moved draws are** isn't measured. What's said is what the last step was worth, which the usual practice takes as the figure, and the notice says how often moves were taken. A run of rarely taken moves is warned of, not corrected.
- **The budget** isn't a setting. Running out of it before the weight is all the way in is a notice that says how far it got, and that what's read is wrong; what's read doesn't say so itself.

## What step 4 is

- **Where:** `take_exactly`, in `chance.rs`, when a reader is given what's known. `Reader::given_by_drawing` never does it, so tests can check one against the other.
- **What's taken:** the parts of what's seen, a sum of logs, that are quadratics (`Quad`) over bell-curve draws, together. What's not a quadratic, or is over a draw that's not a bell curve, is left, and is weighed or moved as in steps 1 and 3, over what's exact. A truth that's known is kept to the same way.
- **The algebra:** precision `P = I - 2A`, Cholesky `P = R·Rᵀ`, middle `P⁻¹b`, and each draw replaced by its middle plus `R⁻ᵀ` times new standard draws, named for the ones they stand in for with `|seen` after, so it's the same every time. At most 400 draws are replaced at once; past that, it's drawn.
- **Reading through it:** a replaced draw's straight-line form is its replacement, its column is that worked out from the new draws, and what it depends on is the new draws, so two things that share none of them are still told apart and two that share one aren't. Nothing else in how anything is read changed.
- **What it says:** a summary or a chance through a replaced draw is `given: exact`, and `exact` where it has a law. The app says *exactly normal(2.099, 0.04328), given what's been seen*, or, where there's no law, *read from draws of exactly what it is given what's been seen*. The notice `evidence_taken_exactly` says how many draws were replaced, and whether any of what's seen was left to weighing or moving.
- **The runners:** the slope is `normal(2.0989115110224503, 0.04327818833887758)` to nine figures against numpy's own, and a new runner's 10k at 22 minutes is `normal(46.1702…, 1.0162…)`, exact, where moving gave 2.099 ± 0.043 from draws.

It leaves out steps 5 to 7.

## How it'd be checked

| Check | How |
|---|---|
| **A truth as evidence is as it was** | Every test and sample from before |
| **An observation over short lists is Bayes' rule** | A coin's bias from tosses, a middle from a reading, and a die from a roll, against the sums done by hand (`tests/seen.rs`) |
| **Weighted reading is honest** | A bell curve seen through a bell curve against its exact answer, 2.2 ± 0.45, and a line through five rows against a grid, 2.004. Forty rows leave a few dozen draws counting (`tests/seen.rs`) |
| **What's seen isn't asked about** | Another roll of 7 moves which die it was by Bayes' rule, with nothing new among the questions (`recollect-server`'s tests) |
| **Moving gets there** | Read by drawing alone, the line through one, five and forty runners against a grid, the forty 2.099 ± 0.043 with σ = 1, middle and spread. Two humps far apart, both kept about evenly (`tests/seen.rs`). Not yet: the tea's six humps, whose inference is over short lists and so never moves |
| **It's repeatable** | The same graph read twice, the same answer to the last figure |
| **Exact is exact** | A bell curve seen through a bell curve, and a line through five points, against the algebra by hand to nine figures, slope and height related as they should be (both above their middles ¼ + asin ρ/2π), and against moving the draws within what drawing allows (`chance.rs`). The line through one, five and forty runners against numpy to nine figures (`tests/seen.rs`). An uncertain spread, a draw that isn't a bell curve, or a square seen, are left to drawing; a truth known too is kept to over what's exact |
