//! Toy: tables whose key has more than one part, as `Cell[2, 3]`, which makes a grid. A grid's rows nobody
//! wrote are there when something reads them, as a numbered table's are, so a recurrence needs only its
//! first cell written. A table can say how far it goes, its extent, and then every row inside it is there,
//! and none outside: a sudoku is a table of choices with nothing written but how far it goes.

mod support;

use recollect_graph::{Log, Op};
use recollect_graph::table::Key;
use recollect_graph::worlds::{Cond, Space, Var, Worldly};
use recollect_systems::expr::Val;
use recollect_systems::formulas::{OPTIONS, Options, REQUIRES, TableExtent, ValueCell, options_of, truth};
use recollect_systems::plan;
use serde_json::{Value, json};
use support::N;

fn put(g: &mut N, name: &str, cell: &str, v: Value) {
    let e = g.e(name);
    g.g.set_value(e, cell, v);
}

fn cell_at(g: &N, e: recollect_graph::Entity, cell: &str) -> Cond<Val> {
    let r = g.g.get_value(e, cell).unwrap().unwrap_or_else(|| panic!("no {cell}"));
    serde_json::from_value::<Worldly<Val>>(r).unwrap().0
}

fn one(c: Cond<Val>) -> Val {
    match c.cases().as_slice() {
        [(_, v)] => (*v).clone(),
        other => panic!("{other:?} isn't one value"),
    }
}

/// What a row of a table says in a cell, written or not.
fn at(g: &N, table: &str, key: Key, cell: &str) -> Val {
    let e = g.g.row_at(table, &key).unwrap_or_else(|| panic!("no row {table}[{key}]"));
    one(cell_at(g, e, cell))
}

fn k(parts: &[i64]) -> Key {
    Key::of(parts.iter().map(|n| Key::Num(*n)).collect())
}

fn value(g: &N, name: &str) -> Result<Val, String> {
    match g.get::<ValueCell>(name) {
        Ok(Some(v)) => Ok(one(v.0.0)),
        other => Err(format!("{other:?}")),
    }
}

#[test]
fn a_grid_goes_on_by_itself_from_what_is_written() {
    let mut g = N::with(plan::install);
    // Pascal's triangle: each is the two above it, and only the top is written. Above the top, and left of
    // the edge, there's nothing, so or(…, 0) says what it is there.
    g.formula_on("P", "c", "or(P[row[0] - 1, row[1] - 1].c, 0) + or(P[row[0] - 1, row[1]].c, 0)");
    put(&mut g, "P[0, 0]", "c", json!(1));
    assert_eq!(at(&g, "P", k(&[4, 2]), "c"), Val::Num(6.0));
    assert_eq!(at(&g, "P", k(&[10, 5]), "c"), Val::Num(252.0));
    assert_eq!(at(&g, "P", k(&[3, 4]), "c"), Val::Num(0.0), "past the end of a line of the triangle");
    let saved = g.g.save();
    assert_eq!(saved.nodes.len(), g.g.nodes().written().count(), "and none of those is written down");

    // `row` is which row this is, a part for each way across, and `row[1]` is one part of it.
    g.formula_on("P", "which", "row");
    g.formula_on("P", "across", "row[1] * 10");
    assert_eq!(at(&g, "P", k(&[4, 2]), "which"), Val::List(vec![Val::Num(4.0), Val::Num(2.0)]));
    assert_eq!(at(&g, "P", k(&[4, 2]), "across"), Val::Num(20.0));

    // A formula can address any of it by where it is.
    g.formula("Middle", "P[6, 3].c");
    assert_eq!(value(&g, "Middle"), Ok(Val::Num(20.0)));
    g.formula("Above", "or(P[-1, 0].c, 7)");
    assert_eq!(value(&g, "Above"), Ok(Val::Num(7.0)), "before the first row there's nothing");
    for (text, says) in [("P[1].c", "no row P[1]"), ("P[1, 2, 3].c", "no row P[1, 2, 3]"), ("P[1, x].c", "no row P[1, x]"), ("P[3, 1].colour", "colour")] {
        g.formula("Bad", text);
        let err = value(&g, "Bad").unwrap_err();
        assert!(err.contains(says), "{text}: {err}");
    }
    g.formula_on("P", "bad", "row[2]");
    let e = g.g.row_at("P", &k(&[1, 1])).unwrap();
    let err = format!("{:?}", g.g.get_value(e, "bad"));
    assert!(err.contains("no part 2"), "{err}");
    g.formula_on("P", "bad", "P[row + 1, 0].c");
    let err = format!("{:?}", g.g.get_value(e, "bad"));
    assert!(err.contains("needs one part"), "{err}");
    g.formula_on("P", "bad", "row + 1");
    let err = format!("{:?}", g.g.get_value(e, "bad"));
    assert!(err.contains("list"), "a list isn't a number: {err}");
    g.clear("P", "bad");
    g.assert_round_trips();
}

#[test]
fn an_extent_says_which_rows_there_are_and_a_star_is_what_is_written() {
    let mut g = N::with(plan::install);
    // Every team written anywhere has each of three rounds.
    g.add("Score", "extent").set("Score", TableExtent("*, 1..3".to_string()));
    g.formula_on("Score", "points", "row[1] * 10");
    g.e("Score[Reds, 1]");
    put(&mut g, "Score[Blues, 2]", "points", json!(5));
    let rows = g.g.rows("Score");
    assert_eq!(rows.len(), 6, "two teams, three rounds");
    g.formula("Teams", "count(Score.rows)");
    assert_eq!(value(&g, "Teams"), Ok(Val::Num(6.0)));
    g.formula("Reds", "sum(Score[Reds, *].points)");
    assert_eq!(value(&g, "Reds"), Ok(Val::Num(60.0)), "a slice of the grid, written or not");
    g.formula("Blues", "sum(t in Score[Blues, 1..2]: t.points)");
    assert_eq!(value(&g, "Blues"), Ok(Val::Num(15.0)), "with what Blues wrote for round 2");
    g.formula("Round", "sum(Score[*, 3].points)");
    assert_eq!(value(&g, "Round"), Ok(Val::Num(60.0)));

    // Outside, there's nothing, by definition.
    g.formula("Outside", "defined(Score[Reds, 4].points) || defined(Score[Greens, 1].points)");
    assert_eq!(value(&g, "Outside"), Ok(Val::Bool(false)));
    // A slice is a group, so it needs something to go over it.
    g.formula("Bad", "Score[Reds, *].points");
    assert!(value(&g, "Bad").unwrap_err().contains("group of rows"));

    // A new team is a new line of the grid.
    g.e("Score[Greens, 3]");
    assert_eq!(value(&g, "Teams"), Ok(Val::Num(9.0)));
    // And changing how far it goes reaches what read it.
    g.set("Score", TableExtent("*, 1..*".to_string()));
    assert_eq!(value(&g, "Teams"), Ok(Val::Num(9.0)), "rounds as far as any written, which is 3");
    g.set("Score", TableExtent("*, 2..3".to_string()));
    assert_eq!(value(&g, "Teams"), Ok(Val::Num(7.0)), "six inside, and Reds' round 1 is written outside");
    assert_eq!(value(&g, "Reds"), Ok(Val::Num(60.0)), "a row written outside is still there to be read by its name");

    let (log, _) = Log::from_saved(g.g.save(), g.registry.clone());
    let said: Vec<String> = log.violations().into_iter().map(|v| format!("{v:?}")).collect();
    assert!(said.iter().any(|v| v.contains("Score[Reds, 1] is written outside")), "{said:?}");
    g.set("Score", TableExtent("*, 1..3".to_string()));
    let (log, _) = Log::from_saved(g.g.save(), g.registry.clone());
    assert!(log.violations().is_empty(), "{:?}", log.violations());

    // How far a table goes is text that says so, and never a formula.
    let s = g.id("Score");
    let (mut log, _) = Log::from_saved(g.g.save(), g.registry.clone());
    let err = format!("{:?}", log.commit("me", "", vec![Op::set_value(s, "extent", json!("9..1"))]).unwrap_err());
    assert!(err.contains("backwards"), "{err}");
    assert!(log.commit("me", "", vec![Op::set_formula(s, "extent", "\"1..3\"")]).is_err());
    assert_eq!(log.graph().extent("Score").map(|e| e.to_string()), Some("*, 1..3".to_string()));
    g.assert_round_trips();
}

/// The choices a grid's solutions range over: one for each of its rows.
fn vars_of(g: &N, table: &str) -> Vec<Var> {
    g.g.rows(table)
        .into_iter()
        .map(|e| {
            let options = options_of(&g.g, e).expect("each row is a choice");
            let options: Vec<&str> = options.iter().map(String::as_str).collect();
            Space::global().uniform(&e.to_string(), &options)
        })
        .collect()
}

fn sudoku(g: &mut N, n: i64, box_size: i64) {
    let options: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
    g.add("Cell", OPTIONS).set("Cell", Options(options));
    g.add("Cell", "extent").set("Cell", TableExtent(format!("1..{n}, 1..{n}")));
    // Each cell differs from the rest of its line, its column and its box: said once, for every cell.
    let start = |i: &str| format!("floor((row[{i}] - 1) / {box_size}) * {box_size} + 1");
    let boxed = |i: &str| format!("{} .. {} + {}", start(i), start(i), box_size - 1);
    g.formula_on(
        "Cell",
        "fits",
        &format!("distinct(Cell[row[0], *].chosen) && distinct(Cell[*, row[1]].chosen) && distinct(Cell[{}, {}].chosen)", boxed("0"), boxed("1")),
    );
    g.formula("Solved", "all(Cell.rows.fits)").add("Solved", REQUIRES);
}

#[test]
fn a_sudoku_is_a_grid_of_choices_with_nothing_written_but_how_far_it_goes() {
    let mut g = N::with(plan::install);
    sudoku(&mut g, 4, 2);
    assert_eq!(g.g.rows("Cell").len(), 16);
    assert_eq!(g.g.nodes().written().count(), 2, "only the table and the clue are written");
    let valid = truth(&g.get::<ValueCell>("Solved").unwrap().unwrap());
    let vars = vars_of(&g, "Cell");
    assert_eq!(valid.count(&vars), 288);
    assert!(options_of(&g.g, g.id("Cell")).is_none(), "the table isn't a choice itself");

    // Each solution is a grid with each number once in every line, column and box, checked here without
    // the formula.
    let names: Vec<(String, Key)> = g.g.rows("Cell").into_iter().map(|e| (e.to_string(), g.g.row_of(e).unwrap().key)).collect();
    let solved = valid.solutions(&vars, 1000);
    assert_eq!(solved.len(), 288);
    let digit = |s: &std::collections::BTreeMap<String, String>, r: i64, c: i64| -> String {
        let (id, _) = names.iter().find(|(_, key)| *key == k(&[r, c])).unwrap();
        s[id].clone()
    };
    for s in &solved {
        for i in 1..=4 {
            let line: std::collections::BTreeSet<String> = (1..=4).map(|c| digit(s, i, c)).collect();
            let column: std::collections::BTreeSet<String> = (1..=4).map(|r| digit(s, r, i)).collect();
            let (br, bc) = ((i - 1) / 2 * 2 + 1, (i - 1) % 2 * 2 + 1);
            let boxed: std::collections::BTreeSet<String> = [(br, bc), (br, bc + 1), (br + 1, bc), (br + 1, bc + 1)].iter().map(|(r, c)| digit(s, *r, *c)).collect();
            assert_eq!((line.len(), column.len(), boxed.len()), (4, 4, 4), "{s:?}");
        }
    }

    // A given is a clue naming a cell by where it is.
    g.formula("Given", "Cell[1, 1].chosen.value = 3 && Cell[2, 3].chosen.value = 1").add("Given", REQUIRES);
    let given = truth(&g.get::<ValueCell>("Given").unwrap().unwrap());
    let both = &valid & &given;
    let by_hand = solved.iter().filter(|s| digit(s, 1, 1) == "3" && digit(s, 2, 3) == "1").count();
    assert!(by_hand > 0 && by_hand < 288);
    assert_eq!(both.count(&vars), by_hand as u128);
    g.assert_round_trips();
}

// KNOWN LIMITATION: this is what a 9×9 should be, but the worlds can't hold it yet. One cell's `fits` (three
// `distinct`s, as pairwise `!=`, over 21 nine-way choices) alone takes about a minute and 78 million nodes,
// and as written the run is killed for memory. Starting from the givens and adding each cell's `fits` in
// turn doesn't help, as it's each `fits` that's too big. It needs `distinct` to be its own constraint in the
// worlds, or choices held some other way. Run it with `cargo test --release -- --ignored` to try.
#[test]
#[ignore = "too big for the worlds as they are: see the note above"]
fn a_nine_by_nine_sudoku_with_its_givens_has_one_solution() {
    let mut g = N::with(plan::install);
    sudoku(&mut g, 9, 3);
    let puzzle = ["53..7....", "6..195...", ".98....6.", "8...6...3", "4..8.3..1", "7...2...6", ".6....28.", "...419..5", "....8..79"];
    let givens: Vec<String> = puzzle
        .iter()
        .enumerate()
        .flat_map(|(r, line)| line.chars().enumerate().filter(|(_, d)| *d != '.').map(move |(c, d)| format!("Cell[{}, {}].chosen.value = {d}", r + 1, c + 1)))
        .collect();
    g.formula("Given", &givens.join(" && ")).add("Given", REQUIRES);
    let started = std::time::Instant::now();
    let valid = truth(&g.get::<ValueCell>("Solved").unwrap().unwrap());
    let given = truth(&g.get::<ValueCell>("Given").unwrap().unwrap());
    let vars = vars_of(&g, "Cell");
    let both = &given & &valid;
    assert_eq!(both.count(&vars), 1);
    let solved = both.solutions(&vars, 2);
    let names: std::collections::BTreeMap<String, Key> = g.g.rows("Cell").into_iter().map(|e| (e.to_string(), g.g.row_of(e).unwrap().key)).collect();
    let first_line: String = (1..=9).map(|c| solved[0].iter().find(|(id, _)| names[*id] == k(&[1, c])).unwrap().1.clone()).collect();
    assert_eq!(first_line, "534678912");
    eprintln!("9×9 worked out in {:?}", started.elapsed());
}

#[test]
fn every_row_inside_an_extent_is_listed_and_writing_one_keeps_it_the_same_node() {
    use recollect_graph::{Graph, Name, NodeKind};
    let mut g = N::with(plan::install);
    sudoku(&mut g, 4, 2);
    // Nothing has read a cell yet, and all sixteen are there, each called by where it is.
    let listed = g.g.nodes();
    let rows: Vec<_> = listed.of(NodeKind::Row).collect();
    assert_eq!(rows.len(), 16);
    assert_eq!(listed.written().count(), 2, "the table and the clue");
    assert_eq!(g.g.get::<Name>(rows[0]).unwrap().unwrap().0, "Cell[1, 1]");
    assert_eq!(rows[0], Graph::row_id("Cell", &k(&[1, 1])));
    assert!(g.g.in_extent(rows[0]));

    // Writing on one makes it a node with the handle it already had, and only then is it saved.
    let cell = Graph::row_id("Cell", &k(&[2, 3]));
    assert!(g.g.make_written(cell));
    assert_eq!(g.g.kind_of(cell), Some(NodeKind::Row), "saying only its name, it's still as if nobody wrote it");
    assert_eq!(g.g.save().nodes.len(), 2);
    g.g.set(cell, Options(vec!["1".to_string()]));
    assert_eq!(g.g.kind_of(cell), Some(NodeKind::Written));
    assert!(g.g.save().nodes.iter().any(|n| n.id == cell));
    assert_eq!(g.g.rows("Cell").len(), 16, "it's still the one row");
    assert_eq!(g.g.nodes().of(NodeKind::Row).count(), 15);
    // Taking away what was written makes it as if nobody wrote it again.
    g.g.clear(cell, OPTIONS);
    assert_eq!(g.g.kind_of(cell), Some(NodeKind::Row));
    assert_eq!(g.g.save().nodes.len(), 2);
    g.assert_round_trips();
}
