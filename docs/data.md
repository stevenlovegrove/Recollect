# Data

**Status:** written down before it's built, as [distributions.md](distributions.md) was. [Order of building](#order-of-building) says which parts are there. The engine's part is in `crates/recollect-graph/src/chance.rs`, the formula language's in `expr.rs` and `formulas.rs`, and reading a CSV in `crates/recollect-core/src/ops.rs`.

## Why

A distribution today is something someone writes: `normal(0.05, 0.01)`, "about three weeks". Often what's known isn't a guess but a table of what happened: forty runners' 5k and 10k times, a year of delivery days, last season's sales by shop. What's wanted from it is a distribution for something not yet seen, and it should come out of the data the way everything else here comes out of its inputs: as a formula, exact, and related to whatever else depends on the same thing.

Columns of real data are rarely unrelated. A runner who's quick over 5k is quick over 10k. A distribution fitted to each column on its own gets every column right and every question about two of them wrong, which is the same mistake [distributions.md](distributions.md#the-idea-what-s-uncertain-is-part-of-the-world) avoids for `x - x`.

## A table of data

Data is a table whose cells are plain values, one row for each thing seen, as any table is ([expressions.md](expressions.md#tables)): `Runners[1].t5k`, `Runners[1].t10k`. Nothing about it is special. It can be typed in, written by Claude with the `table` op, or read from a CSV.

### Reading a CSV

The `table` op takes `"csv"` in place of `"rows"`:

```json
{"op": "table", "name": "Runners", "csv": "t5k,t10k,age\n21.5,44.9,34\n…", "key": "id"}
```

- **The first line names the columns.** Each name is what formulas call that cell, so it has to read as a name: spaces and dashes become `_`, and anything else that can't be a name is refused, naming it.
- **Each line after it is a row.** A number is a number, `true` and `false` are truths, and anything else is text. An empty field writes nothing, so that row doesn't have that cell.
- **The rows are numbered from 1,** unless `"key"` names a column, whose values are then the keys and not a cell.
- Fields can be quoted, as `"Smith, J"`, with `""` for a quote inside one.

In the app, a CSV file is read the same way, from **Import a CSV as a table…** in the workspace menu, which asks what to call the table.

## A row as one draw

```
Pick: any_row(Runners.rows)
```

`any_row` is one row of a table, each as likely as the others. It's a short list, as `integers(1, 6)` is, so it's a variable in the diagram, and everything worked out from it is counted, exactly. What it lands on is a row, as a choice lands on an option, so its cells are read through it: `pick.value.t5k`.

- **Every column read through one pick moves together.** `pick.value.t5k` and `pick.value.t10k` are the same runner's, so their joint distribution is the data's own, with nothing fitted and nothing lost.
- **Conditioning is evidence, as it already is.** `evidence: pick.value.t5k < 22` leaves the runners quicker than 22 minutes, and `pick.value.t10k` is then what they ran over 10k. Pinning a range on a histogram is the same, so brushing one column's chart shows the others' given it.
- **Each place it's written is its own pick,** as for any draw. Two picks are two runners, drawn separately.
- **It's for data that's plain.** A row whose cells differ between worlds or are uncertain is read as it is, world by world, but the histogram of a column is then a mixture, and says so.
- **A table of more than 1,000 rows** is refused, as `integers` is past 1,000 numbers, with the reason. Drawing a row by a uniform draw would lift that, and isn't built.

This answers "what's in the data". It doesn't answer "what would the next runner do", which is a question about runners in general: the forty seen are a sample, and how much they say depends on how many there are.

## Fitting, as inference

```
predict(r in Runners.rows: r.t10k)
predict(r in Runners.rows: r.t10k, r.t5k)
predict(r in Runners.rows: r.t10k, r.t5k, r.age)
```

`predict(r in T: y, x₁, x₂, …)` is the distribution of `y` for something new, given the rows of `T`, and given its own `x`s. The `x`s for the new thing are the same expressions read on the row the formula's written on: `r` stands for `self` there. So written as a column of a table of runners not yet seen, `predict(r in Runners.rows: r.t10k, r.t5k)` is, on each row, what that runner's 10k is likely to be given their 5k and the forty runners seen. With no `x`s it's what any new `y` is likely to be.

### What it is

It's a straight line with a bell curve about it, `y = b₀ + b₁x₁ + … + noise`, fitted by inference rather than by picking the best line:

- **The line and the spread are uncertain,** and what the data says about them is worked out exactly: with nothing known beforehand (the usual reference prior), the spread squared is the residual spread times `ν / χ²ν`, where `ν` is the number of rows less the number of things fitted, and the line, given the spread, is a bell curve about the least-squares line, as wide as the data leaves it.
- **A new `y` is the line at its `x`s, plus noise of that spread.** So the prediction is wider than the scatter about the line by as much as the line itself is unsure, which is a lot with five rows and hardly anything with five thousand, and wider again far from where the data is.
- **It's held as a formula over draws, like any distribution.** A `χ²` draw for the spread, a standard normal for each part of the line, and one for the new thing's own noise. Nothing is approximated by holding it: the tree is exact, and what's read from it says how it was got.

That's what "fitting as inference" means here. Writing the priors as cells, `mu = normal(…)`, and each row as a piece of evidence that it came from `normal(mu, sigma)`, says the same thing, and [Not yet](#not-yet) says why it isn't how it's worked out.

### What's shared

The line and the spread are what the data says, so they're named for the data and not for where `predict` is written: every `predict` over the same rows and the same expressions reads the same line and spread. Each one's own noise is its own, named for where it's written, as any draw is.

So a table of predictions is right together, and not only one at a time. Two new runners' predictions are related, because both lean on the same uncertain line, and a total of predictions is as wide as it should be. Fitting each separately would make the total too narrow.

### What it needs

- **Plain numbers.** Each row's `y` and `x`s have to be a plain number, the same in every world. A row that has none, or text, or something uncertain, is an error that names it, never a row quietly left out. `or()` or a filtered table says otherwise.
- **More rows than things fitted.** At least one more, and at least two to say anything about the spread. With too few it's an error, and with `x`s that are one another's multiples, as a column in minutes and another in seconds, there's no one line, and that's an error too.
- **Its `x`s on the row it's written on.** One that's undefined there makes the prediction undefined there. An `x` that's uncertain is fine: the prediction carries it.

### Shapes other than a line

The expressions are formulas, so the usual changes of shape are written, not built:

| For | Write |
|---|---|
| Something that grows by multiplying, never below 0 | `exp(predict(r in T: ln(r.y), ln(r.x)))`: a lognormal about a power law |
| A curve | `predict(r in T: r.y, r.x, r.x ^ 2)` |
| A difference between groups | `predict(r in T: r.y, if(r.group = "B", 1, 0))` |

## Seeing how well it does

A hold-out table is rows that weren't fitted to, with the answer known. A column of predictions beside the answer says how good each model is, and every score is an ordinary formula:

| Score | Formula | What it says |
|---|---|---|
| Where the answer fell | `chance(pred < self.t10k)` | Spread evenly from 0 to 1 for a model that's honest about how unsure it is. Bunched in the middle, it's too wide. Piled at the ends, too narrow |
| Inside the 90% range | `abs(chance(pred < self.t10k) - 0.5) < 0.45` | About nine rows in ten, for an honest model |
| How far out the middle was | `abs(mean(pred) - self.t10k)` | Smaller is sharper |

An honest model that ignores the 5k time is inside its range as often as it should be, and far out in the middle. One that reads it is as honest, and much sharper. That's the difference between a column's distribution and a runner's.

## Ways to look at it

The views should show data and what's fitted to it together. These are the ideas, none built yet:

- **A grid of pairs.** Each column's histogram on the diagonal and each pair's scatter off it, with a fitted line, or a prediction's fan, over the scatter, so where they disagree is seen.
- **Brushing.** Dragging over one histogram pins a range on a pick, and every other histogram becomes what it is given that, with what it was faded behind.
- **Correlations.** A grid of how related each pair of columns is, by value and by rank. Where the two differ, the relation isn't a straight line, which is worth a notice.
- **A fan down a hold-out table,** each row's prediction against its answer.
- **Where the answers fell,** as a histogram of the first score above, for each model.

## Not yet

- **Writing priors as cells, and data as evidence for them,** is the general form of fitting. Here, evidence is held by keeping the draws where it holds, which works for a reading that's a few degrees wide, as in the tea. A data point is a number exactly, so no draw meets it, and the general way to condition on it is to weight each draw by how likely it makes the data. With forty rows almost every one of 4,096 draws would then weigh next to nothing, and what's read would be a few draws dressed up as many. So until there's a way of drawing that follows the data (weights that say how few draws they really are, at the least), fitting is done where the answer is known exactly, as `predict` is.
- **A prior of your own** for `predict`, as weights on what's believed beforehand, isn't built. The reference prior is said by name wherever a prediction is described.
- **A prediction has no law,** so what's read from it is read from 4,096 draws, and says so. Alone, it's a Student t, which has a formula. Recognising one would make a single prediction's chances exact.
- **More than one `y` at once,** a bell curve over several columns together, isn't built: predicting each from the others is.
- **How likely the data is under a model** (a log score, or comparing models by evidence) needs a density, which only laws have.
- **Fitting other shapes** than a bell curve about a line, such as an exponential wait or a count, isn't built.

## Order of building

| Step | What | State |
|---|---|---|
| 1 | Reading a CSV into a table, in the `table` op and from the workspace menu | Not built |
| 2 | `any_row`: a row as one draw | Not built |
| 3 | `predict`, by exact inference, its line and spread shared by everything fitted to the same data | Not built |
| 4 | The runners sample, with a hold-out table and its scores | Not built |
| 5 | The views above | Not built |

## How it's checked

| Check | How |
|---|---|
| **A CSV is its table** | Quoting, empty fields, keys, and names that can't be names, each read as the table op would write them by hand |
| **A pick is the data** | Its mean and spread are the column's, two columns through one pick have the data's correlation, and evidence on one leaves the rows it should |
| **A prediction is the textbook's** | With no `x`s, its spread is the Student t's, `s·√(1 + 1/n)` with `n - 1` degrees of freedom. With some, its middle is the least-squares line at the new `x`s |
| **What's shared is shared** | Two predictions from the same data are related, and their difference is narrower than if they weren't |
| **Data that isn't plain is refused** | A missing cell, text, or an uncertain number, each an error naming the row |

## The sample

**Runners, given their 5k** has forty runners' 5k and 10k times and ages, and ten more not fitted to. Each of the ten has three predictions of their 10k: from nothing, from their 5k, and from their 5k on a log scale. Their scores say all three are honest and that the 5k makes the prediction several times sharper. `tests/samples.rs` checks it says what it claims.
