# Data

**Status:** reading a CSV and `any_row` are built. Fitting a model to data is written as ordinary cells, and [Fitting](#fitting) says what the engine makes of that today. The views aren't built. `any_row` is in `crates/recollect-systems/src/formulas.rs`, and reading a CSV in `crates/recollect-core/src/ops.rs`.

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

There's no fitting function. A model is written in the workspace as equations, as anything else is: its parameters as cells with distributions saying what's believed before the data, and the data's relation to them as formulas on the rows.

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

## The sample

**Runners, row by row** has forty runners' 5k and 10k times. One picked at random is a row, so knowing their 5k was under 22 minutes says their 10k was under 47.
