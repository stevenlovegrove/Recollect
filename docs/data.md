# Data

**Status:** reading a CSV and `one_of` are built. Fitting a model to data is written as ordinary cells, and [Fitting](#fitting) says what the engine makes of that today, and what it would take to fit the way that's meant. The views aren't built. `one_of` is in `crates/recollect-systems/src/formulas.rs`, and reading a CSV in `crates/recollect-core/src/ops.rs`.

## Why

A distribution today is something someone writes: `normal(0.05, 0.01)`, "about three weeks". Often what's known isn't a guess but a table of what happened: forty runners' 5k and 10k times, a year of delivery days, last season's sales by shop. What's wanted from it should come out of the data the way everything else here comes out of its inputs: as formulas, exact where they can be, and related to whatever else depends on the same thing.

Columns of real data are rarely unrelated. A runner who's quick over 5k is quick over 10k. A distribution taken from each column on its own gets every column right and every question about two of them wrong, which is the same mistake [distributions.md](distributions.md) avoids for `x - x`.

## A table of data

Data is a table whose cells are plain values, one row for each thing seen, as any table is ([expressions.md](expressions.md#tables)): `Runners[1].t5k`, `Runners[1].t10k`. Nothing about it is special. It can be typed in, written by Claude with the `table` op, or read from a CSV.

### Reading a CSV

The `table` op takes `"csv"` in place of `"rows"`:

```json
{"op": "table", "name": "Runners", "csv": "t5k,t10k,age\n21.5,44.9,34\n…", "key": "id"}
```

- **The first line names the columns.** Each name is what formulas call that cell, so it has to read as a name: spaces and dashes become `_`, and anything else that can't be a name, such as `5k`, is refused, naming it.
- **Each line after it is a row.** A number is a number, `true` and `false` are truths, and anything else is text. An empty field writes nothing, so that row doesn't have that cell.
- **The rows are numbered from 1,** unless `"key"` names a column, whose values are then the keys and not a cell.
- Fields can be quoted, as `"Smith, J"`, with `""` for a quote inside one.

In the app, a CSV file is read the same way, from **Import a CSV as a table…** in the workspace menu, which asks what to call the table and suggests a name a formula can say.

## A row as one draw

```
Pick: one_of(Runners.rows)
```

`one_of` is one of a group of nodes, each as likely: here, one row of a table. It's a short list, as `integers(1, 6)` is, so it's a variable in the diagram, and everything worked out from it is counted, exactly. What it lands on is a row, as a choice lands on an option, so its cells are read through it: `pick.value.t5k`.

- **Every column read through one pick moves together.** `pick.value.t5k` and `pick.value.t10k` are the same runner's, so their joint distribution is the data's own, with nothing fitted and nothing lost.
- **Conditioning is evidence, as it already is.** `evidence: pick.value.t5k < 22` leaves the runners quicker than 22 minutes, and `pick.value.t10k` is then what they ran over 10k.
- **Each place it's written is its own pick,** as for any draw. Two picks are two runners, drawn separately.
- **It takes any group of nodes,** as a fold does: `Runners.rows`, or a name several nodes are labelled. Anything else is an error, and so is a group with nothing in it.
- **A group of more than 1,000** is refused, as `integers` is past 1,000 numbers, with the reason.

This answers "what's in the data". It doesn't answer "what would the next runner do", which is a question about runners in general: the forty seen are a sample, and how much they say depends on how many there are. That's fitting.

## Fitting

There's no fitting function. A model is written in the workspace as equations, as anything else is: its parameters as cells with distributions saying what's believed before the data, and the data's relation to them as formulas on the rows. A straight line through the runners:

```
m:          normal(2, 0.5)                    what's believed of the slope beforehand
c:          normal(0, 5)                      and of the height
T.residual: self.y - (m * self.x + c)         a column: each row's miss, for each m and c
Fits:       all(r in T.rows: abs(r.residual) < 5)     evidence
```

### What the engine makes of it

Every part of that means what it says. `m` and `c` are draws, each row's `residual` is an expression over the same two, and `Fits` is one uncertain truth over them. As evidence, it leaves only the draws where every residual is within 5, and everything that reads `m` or `c` is read from those, saying how many they were.

| Rows | Within | Draws left of 4,096 | Slope, as read | Slope, exactly (a grid) |
|---|---|---|---|---|
| 1 | ±2 | 474 | 2.044 ± 0.18 | 2.054 ± 0.19 |
| 10 | ±2 | 53 | 2.140 ± 0.08 | 2.130 ± 0.08 |
| 40 | ±5 | 499 | 2.098 ± 0.14 | 2.098 ± 0.15 |
| 40 | ±2 | 0, so undefined | none | none: no line fits all forty |

So the engine is right about the model it was given. What's wrong is the model: it isn't the one meant.

- **Evidence is a truth, so a residual can only be in or out.** "Every runner is within 5 minutes of the line" is a box: every line that keeps them all inside it is as good as any other. It's why the slope comes out ±0.14 from forty points, about what it was from one.
- **What's meant is that residuals are spread like a bell curve,** so a line that misses by a little is likelier than one that misses by a lot. That's how likely the data is under each `m` and `c`, a weight, not a truth, and there's no way to write it. `evidence` drops draws; nothing weighs them.
- **And weighing 4,096 draws from the prior wouldn't be enough.** With the bell curve's spread at 1 minute, the slope given forty runners is 2.099 ± 0.043, a third as wide. Weighting the prior's draws by how likely they make the data leaves the equivalent of about 22 of them, and about 6 with a spread of half a minute. Read honestly, that's a value saying it was got from 22 draws. Read as 4,096, it's wrong without saying so.
- **Narrowing the box doesn't get there.** It runs out of draws (10 rows within ±1 leaves none), and then out of lines (40 within ±2 has none at all), before it says anything a bell curve would.

### A residual for each row, drawn

The other way to write it is the model as it's usually stated: each row is the line plus a residual of its own, drawn from a bell curve, in a table beside the data.

```
E.e:   normal(0, 1)                                      a column: each row its own draw
Fits:  all(r in T.rows: r.y = m * r.x + c + E[r.row].e)   evidence
```

That's cheap to hold: forty rows are 456 expressions and a quarter of a second. It fails at the first row, and not for cost. `r.y = m * r.x + c + E[r.row].e` asks whether an uncertain number is exactly another, which it never is. For one row the engine says so exactly, since the difference has a law. For two or more it's read from draws, as "0 in 4,096". Either way no draw is left, and the slope is undefined.

Loosened to `abs(r.y - (m * r.x + c + E[r.row].e)) < tol`, it's the right model. As `tol` narrows, what it leaves is exactly the bell curve's answer, where the box round the line alone is a different model however narrow it gets. The engine reads it as a grid over the same prior and evidence says it should:

| Rows | Within | Draws left, read | Draws left, expected | Slope, exactly at that width | Bell curve's slope |
|---|---|---|---|---|---|
| 1 | ±1 | 253 | 234.6 | 2.054 ± 0.185 | 2.054 ± 0.184 |
| 2 | ±1 | 86 | 92.7 | 2.017 ± 0.161 | 2.008 ± 0.151 |
| 3 | ±1 | 40 | 44.1 | 1.992 ± 0.152 | 1.979 ± 0.140 |
| 4 | ±1 | 20 | 20.5 | 2.026 ± 0.130 | 2.019 ± 0.118 |
| 5 | ±1 | 11 | 11.2 | 2.011 ± 0.121 | 2.004 ± 0.109 |
| 6 | ±1 | 1 | 0.9 | 2.161 ± 0.090 | 2.163 ± 0.081 |
| 3 | ±0.5 | 1 | 6.3 | 1.982 ± 0.143 | |

Each row keeps the share of draws whose residual happens to land within `tol` of the one that fits, about `2·tol` times the bell curve's height there: a half or less at ±1, and an eighth at ±0.25. Draws left fall by that much again with every row, so six rows at ±1 leave one, and all forty would need about 3 × 10¹⁶ draws for each one left at ±1, and 10⁴¹ at ±0.25. Narrowing `tol` to get closer to the bell curve makes it worse. It breaks on drawing, not on holding.

Read with what's left, every answer says how few draws it came from, so none of this is wrong without saying so: "2.215 ± 0.001" from two draws says it's from two.

It's the same trade as before. The engine can say what's true of a model with a truth, and the only cure for its running out of draws is not dropping them: weighing each by how likely it makes each row, as below. A residual for each row drawn and then pinned near where it has to be is that weight, got by dropping instead.

### What it would take

Two things, each of use beyond fitting, designed in [inference.md](inference.md):

1. **A way to say how likely something observed is,** as a weight rather than a truth: say `likely(r.residual, normal(0, sigma))`, whose meaning is the bell curve's density at each draw's residual, multiplied over the rows. It would sit beside `evidence`, as `evidence` sits beside `requires`, because it's a third thing: not "this is so" but "this is how likely what was seen is".
2. **Draws that follow the weights,** so what's read after them is a few thousand that count, not a few that count for a lot: drawing again near the draws that weigh the most, or a Markov chain over the draws. Either way the reading has to say how many independent draws it really is, which a weighted reading can work out.

Where `m` and `c` have laws and the residuals are bell curves about a straight line, as here, the answer has a law too (a bell curve over the two of them), and that could be recognised exactly, as sums of bell curves are now. That's a special case of the general thing, not a replacement for it.

## Ways to look at it

The views should show data and what's fitted to it together. These are ideas, none built yet:

- **A grid of pairs.** Each column's histogram on the diagonal and each pair's scatter off it.
- **Brushing.** Dragging over one histogram pins a range on a pick, and every other histogram becomes what it is given that, with what it was faded behind.
- **Correlations.** A grid of how related each pair of columns is, by value and by rank. Where the two differ, the relation isn't a straight line, which is worth a notice.

## How it's checked

| Check | How |
|---|---|
| **A CSV is its table** | Quoting, empty fields, keys, and names that can't be names, each read as the table op would write them by hand |
| **A pick is the data** | Each row as often as the others, two columns through one pick keep each row's pairing, and evidence on one leaves the rows it should |
| **A line, as written, is read honestly** | Evidence about residuals leaves the draws a grid over the same prior says it should, and none where no line fits |
| **A residual drawn for each row** | Exactly equal leaves nothing, within half a minute one row leaves what a grid says, and five rows leave nothing |

## The sample

**Runners, row by row** has forty runners' 5k and 10k times. One picked at random is a row, so knowing their 5k was under 22 minutes says their 10k was under 47.
