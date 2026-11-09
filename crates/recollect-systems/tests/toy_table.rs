//! Toy: tables. A node named `Year[3]` is row 3 of `Year`, and takes the formulas it doesn't have
//! from the node named `Year`. A formula there reads the row before as `prev`, which is how a
//! spreadsheet's filled-down column works, and a choice part way down changes what comes after it.

mod support;

use recollect_graph::worlds::{Cond, Worldly};
use recollect_systems::expr::Val;
use recollect_systems::formulas::ValueCell;
use recollect_systems::plan::{self, ALTERNATIVE, DECISION};
use serde_json::{Value, json};
use support::N;

/// A formula on one of the workspace's own cells.
fn column(g: &mut N, name: &str, component: &str, text: &str) {
    g.formula_on(name, component, text);
}

fn put(g: &mut N, name: &str, component: &str, v: Value) {
    let e = g.e(name);
    g.g.set_value(e, component, v);
}

/// What a cell is, in each world.
fn cell(g: &N, name: &str, component: &str) -> Cond<Val> {
    let r = g.g.get_value(g.id(name), component).unwrap().unwrap_or_else(|| panic!("{name} has no {component}"));
    serde_json::from_value::<Worldly<Val>>(r).unwrap().0
}

fn number(g: &N, name: &str, component: &str) -> f64 {
    match cell(g, name, component).cases().as_slice() {
        [(_, Val::Num(n))] => *n,
        other => panic!("{name}.{component} is {other:?}"),
    }
}

fn years(g: &mut N) {
    g.value("Rate", ValueCell::num(0.05));
    column(g, "Year", "balance", "prev.balance * (1 + rate)");
    g.input("Year", "Rate", "rate");
    put(g, "Year[0]", "balance", json!(1000));
    for i in 1..=10 {
        g.e(&format!("Year[{i}]"));
    }
}

#[test]
fn a_tables_formula_runs_for_each_row_from_the_one_before() {
    let mut g = N::with(plan::install);
    years(&mut g);
    assert!((number(&g, "Year[10]", "balance") - 1628.894626777442).abs() < 1e-6);
    assert!(cell(&g, "Year", "balance").cases().iter().all(|(_, v)| **v == Val::Undefined), "the table itself has no row before it");

    g.take_ran();
    g.value("Rate", ValueCell::num(0.1));
    assert!((number(&g, "Year[10]", "balance") - 2593.74246).abs() < 1e-3);
    assert_eq!(g.ran_cells().iter().filter(|c| c.starts_with("balance(")).count(), 10, "each row once");

    // A gap is a row nobody wrote, which takes the table's formula like the rest, so nothing changes.
    let four = g.id("Year[4]");
    g.g.set(four, recollect_graph::Name("Spare".to_string()));
    assert!((number(&g, "Year[10]", "balance") - 2593.74246).abs() < 1e-3);
    g.g.set(four, recollect_graph::Name("Year[4]".to_string()));
    assert!((number(&g, "Year[10]", "balance") - 2593.74246).abs() < 1e-3);
    g.assert_round_trips();
}

#[test]
fn prev_prev_next_row_and_rows() {
    let mut g = N::with(plan::install);
    column(&mut g, "F", "f", "prev.f + prev.prev.f");
    put(&mut g, "F[0]", "f", json!(0));
    put(&mut g, "F[1]", "f", json!(1));
    for i in 2..=10 {
        g.e(&format!("F[{i}]"));
    }
    assert_eq!(number(&g, "F[10]", "f"), 55.0);

    // `next` runs the other way, and `row` is which row this is.
    column(&mut g, "Countdown", "left", "next.left + 1");
    put(&mut g, "Countdown[3]", "left", json!(0));
    column(&mut g, "Countdown", "doubled", "row * 2");
    for i in 0..3 {
        g.e(&format!("Countdown[{i}]"));
    }
    assert_eq!(number(&g, "Countdown[0]", "left"), 3.0);
    assert_eq!(number(&g, "Countdown[2]", "doubled"), 4.0);

    // `rows` is every row of the table, so a total can be a column too.
    column(&mut g, "Countdown", "total", "sum(rows.doubled)");
    assert_eq!(number(&g, "Countdown", "total"), 12.0, "0 + 2 + 4 + 6");
    assert_eq!(number(&g, "Countdown[1]", "total"), 12.0);
    g.e("Countdown[4]");
    assert_eq!(number(&g, "Countdown", "total"), 20.0, "a new row is counted");

    // A key that isn't a number is still which row this is.
    column(&mut g, "Fruit", "which", "row");
    g.e("Fruit[apple]");
    assert_eq!(cell(&g, "Fruit[apple]", "which").cases()[0].1, &Val::Text("apple".to_string()));
    assert_eq!(cell(&g, "Fruit[apple]", "which").cases().len(), 1);
}

#[test]
fn a_choice_part_way_down_a_table_changes_only_what_comes_after() {
    let mut g = N::with(plan::install);
    years(&mut g);
    g.add("Top up?", DECISION);
    for (option, amount) in [("No", 0.0), ("Yes", 500.0)] {
        g.value(option, ValueCell::num(amount)).link(option, "Top up?", ALTERNATIVE);
    }
    // Year 5 says its own formula, with the top-up the decision lands on in it, and the rest follow on from it.
    column(&mut g, "Year[5]", "balance", "prev.balance * (1 + rate) + top_up.chosen.value");
    g.input("Year[5]", "Rate", "rate").input("Year[5]", "Top up?", "top_up");
    assert_eq!(cell(&g, "Year[4]", "balance").cases().len(), 1, "before the choice, one value");
    let last = cell(&g, "Year[10]", "balance");
    assert_eq!(last.cases().len(), 2, "after it, one for each option");
    let at = |v: &Val| if let Val::Num(n) = v { *n } else { panic!("{v:?}") };
    let (lo, hi) = (last.cases().iter().map(|(_, v)| at(v)).fold(f64::MAX, f64::min), last.cases().iter().map(|(_, v)| at(v)).fold(f64::MIN, f64::max));
    assert!((lo - 1628.894626777442).abs() < 1e-6);
    assert!((hi - lo - 500.0 * 1.05f64.powi(5)).abs() < 1e-6, "the top-up grows for the five years after");
}

#[test]
fn a_formula_can_address_any_row_and_a_numbered_table_goes_on_by_itself() {
    let mut g = N::with(plan::install);
    years(&mut g);
    // Year[100] was never written. Asking for it works out every year up to it from the table.
    g.formula("Far", "Year[100].balance");
    assert!((amount(&g, "Far") - 1000.0 * 1.05f64.powi(100)).abs() < 1e-6);
    let saved = g.g.save();
    assert_eq!(saved.nodes.len(), g.g.entities().count(), "and none of those years is written down");
    g.take_ran();
    amount(&g, "Far");
    assert!(g.ran().is_empty(), "they're remembered");
    g.value("Rate", ValueCell::num(0.1));
    assert!((amount(&g, "Far") - 1000.0 * 1.1f64.powi(100)).abs() < 1e-3, "and changes reach them");

    // Year[row - 1] is the same as prev, and a key can be a name.
    column(&mut g, "Year", "before", "Year[row - 1].balance");
    assert!((number(&g, "Year[3]", "before") - number(&g, "Year[2]", "balance")).abs() < 1e-9);
    put(&mut g, "Car[Hatchback]", "price", json!(18000));
    g.formula("Price", "Car[Hatchback].price + Car[\"Hatchback\"].price");
    assert_eq!(amount(&g, "Price"), 36000.0);

    // What a table doesn't say, a row nobody wrote can't have, and before the first row there's nothing.
    for (text, says) in [("Year[100].colour", "colour"), ("Year[-1].balance", "no row Year[-1]"), ("Car[Estate].price", "no row Car[Estate]"), ("Year[Rate].balance", "no row Year[Rate]")] {
        g.formula("Bad", text);
        let err = format!("{:?}", g.get::<ValueCell>("Bad"));
        assert!(err.contains(says), "{text}: {err}");
    }
    g.assert_round_trips();
}

fn amount(g: &N, name: &str) -> f64 {
    match g.get::<ValueCell>(name).unwrap().unwrap().0.0.cases().as_slice() {
        [(_, Val::Num(n))] => *n,
        other => panic!("{name} is {other:?}"),
    }
}
