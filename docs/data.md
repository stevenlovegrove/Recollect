# Data

**Status:** reading a CSV and `any_row` are built. Fitting a model to data is written as ordinary cells, and [Fitting](#fitting) says what the engine makes of that today, and what it would take to fit the way that's meant. The views aren't built. `any_row` is in `crates/recollect-systems/src/formulas.rs`, and reading a CSV in `crates/recollect-core/src/ops.rs`.

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
Pick: any_row(Runners.rows)
```

`any_row` is one row of a table, each as likely. It's a short list, as `integers(1, 6)` is, so it's a variable in the diagram, and everything worked out from it is counted, exactly. What it lands on is a row, as a choice lands on an option, so its cells are read through it: `pick.value.t5k`.

- **Every column read through one pick moves together.** `pick.value.t5k` and `pick.value.t10k` are the same runner's, so their joint distribution is the data's own, with nothing fitted and nothing lost.
- **Conditioning is evidence, as it already is.** `evidence: pick.value.t5k < 22` leaves the runners quicker than 22 minutes, and `pick.value.t10k` is then what they ran over 10k.
- **Each place it's written is its own pick,** as for any draw. Two picks are two runners, drawn separately.
- **It needs the rows of a table,** as `any_row(Runners.rows)`. Anything else is an error, and so is a table with no rows.
- **A table of more than 1,000 rows** is refused, as `integers` is past 1,000 numbers, with the reason.

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
- **And weighing 4,096 draws from the prior wouldn't be enough.** With the bell curve's spread at 1 minute, the slope given forty runners is 2.099 ± 0.043, a tenth as wide as before. Weighting the prior's draws by how likely they make the data leaves the equivalent of about 22 of them, and about 6 with a spread of half a minute. Read honestly, that's a value saying it was got from 22 draws. Read as 4,096, it's wrong without saying so.
- **Narrowing the box doesn't get there.** It runs out of draws (10 rows within ±1 leaves none), and then out of lines (40 within ±2 has none at all), before it says anything a bell curve would.

### What it would take

Two things, each of use beyond fitting:

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

## The sample

**Runners, row by row** has forty runners' 5k and 10k times. One picked at random is a row, so knowing their 5k was under 22 minutes says their 10k was under 47.
