//! Toy 1: the maths you'd normally use a spreadsheet for. Each cell is either a typed-in number or
//! a formula over what it has labelled.

mod support;

use recollect_graph::{Backed, Error, Registry};
use recollect_systems::expr::Val;
use recollect_systems::formulas::{LABEL, ValueCell, install};
use recollect_systems::plan::{self, ALTERNATIVE, DECISION, PART_OF, TASK};
use serde_json::{Value, json};
use support::N;

fn sheet(reg: &mut Registry) {
    install(reg);
}

fn num(n: f64) -> ValueCell {
    ValueCell::num(n)
}

fn amount(g: &N, id: &str) -> f64 {
    let v = g.get::<ValueCell>(id).unwrap().unwrap();
    let cases = v.0.0.cases();
    assert_eq!(cases.len(), 1, "{id} is one number in every world");
    match cases[0].1 {
        Val::Num(n) => *n,
        other => panic!("{id} is {other:?}"),
    }
}

/// Two invoice lines, a subtotal, tax on it and a total.
fn invoice() -> N {
    let mut g = N::with(sheet);
    for (line, qty, price) in [("widgets", 3.0, 10.0), ("gears", 2.0, 25.0)] {
        let (q, p) = (format!("{line}.qty"), format!("{line}.price"));
        g.value(&q, num(qty)).value(&p, num(price));
        g.formula(line, "quantity * price").input(line, &q, "quantity").input(line, &p, "price");
        g.input("subtotal", line, "line");
    }
    g.formula("subtotal", "sum(line)");
    g.value("tax_rate", num(0.2));
    g.formula("tax", "subtotal * rate").input("tax", "subtotal", "subtotal").input("tax", "tax_rate", "rate");
    g.formula("total", "subtotal + tax").input("total", "subtotal", "subtotal").input("total", "tax", "tax");
    g
}

#[test]
fn an_invoice_adds_up() {
    let g = invoice();
    assert_eq!(amount(&g, "widgets"), 30.0);
    assert_eq!(amount(&g, "subtotal"), 80.0);
    assert_eq!(amount(&g, "total"), 96.0);
    g.assert_round_trips();
}

#[test]
fn editing_one_number_reruns_only_what_it_reaches() {
    let mut g = invoice();
    amount(&g, "total");
    g.take_ran();

    amount(&g, "total");
    assert!(g.ran().is_empty(), "nothing changed, nothing runs");

    g.value("widgets.price", num(11.0));
    assert_eq!(amount(&g, "total"), 99.6);
    let cells = g.ran_cells();
    let mut values: Vec<&String> = cells.iter().filter(|c| c.starts_with("value(")).collect();
    values.sort();
    assert_eq!(values, vec!["value(subtotal)", "value(tax)", "value(total)", "value(widgets)"], "gears doesn't run");
    assert!(cells.iter().filter(|c| c.starts_with("value_type(")).count() <= 1, "and no formula's type has to be worked out again: {cells:?}");

    g.value("widgets.price", num(11.0));
    amount(&g, "total");
    assert!(g.ran().is_empty(), "the same value isn't a change");
}

#[test]
fn a_recomputed_value_that_comes_out_the_same_stops_there() {
    // Swap two factors so a line's value is unchanged: nothing after it should run.
    let mut g = invoice();
    amount(&g, "total");
    g.take_ran();
    g.value("widgets.qty", num(6.0)).value("widgets.price", num(5.0));
    assert_eq!(amount(&g, "total"), 96.0);
    assert_eq!(g.ran_values(), vec!["widgets"], "still 30, so the subtotal never reruns");
}

#[test]
fn asking_for_one_cell_runs_only_what_it_needs() {
    let g = invoice();
    amount(&g, "widgets");
    assert_eq!(g.ran_values(), vec!["widgets"]);
}

#[test]
fn a_cell_can_switch_between_a_number_and_a_formula() {
    let mut g = invoice();
    assert_eq!(amount(&g, "total"), 96.0);
    g.value("subtotal", num(1000.0));
    assert_eq!(amount(&g, "total"), 1200.0, "typed over, as in a spreadsheet");
    assert_eq!(g.backed(g.id("subtotal"), "value").unwrap().0, Backed::Value(json!(1000.0)));
    g.formula("subtotal", "sum(line)");
    assert_eq!(amount(&g, "total"), 96.0, "and back");
    assert_eq!(g.backed(g.id("subtotal"), "value").unwrap().0, Backed::Formula("sum(line)".to_string()));
}

#[test]
fn fibonacci_is_linear_because_shared_cells_run_once() {
    let mut g = N::with(sheet);
    g.value("f0", num(0.0)).value("f1", num(1.0));
    for i in 2..=80 {
        let id = format!("f{i}");
        g.formula(&id, "a + b").input(&id, &format!("f{}", i - 1), "a").input(&id, &format!("f{}", i - 2), "b");
    }
    assert_eq!(amount(&g, "f80"), 23416728348467685.0);
    assert_eq!(g.ran_values().len(), 79, "each once, where naive recursion would take about 10^16 calls");
    let loaded = g.assert_round_trips();
    assert_eq!(loaded.get::<ValueCell>(g.id("f80")).unwrap().unwrap(), num(23416728348467685.0));
}

#[test]
fn compound_interest_is_a_chain_with_a_shared_rate() {
    let mut g = N::with(sheet);
    g.value("y0", num(1000.0)).value("rate", num(0.05));
    for i in 1..=10 {
        let id = format!("y{i}");
        g.formula(&id, "last * (1 + rate)").input(&id, &format!("y{}", i - 1), "last").input(&id, "rate", "rate");
    }
    assert!((amount(&g, "y10") - 1628.894626777442).abs() < 1e-6);
    g.take_ran();
    g.value("rate", num(0.05));
    amount(&g, "y10");
    assert!(g.ran().is_empty());
    g.value("rate", num(0.1));
    assert!((amount(&g, "y10") - 2593.74246).abs() < 1e-3);
    assert_eq!(g.ran_values().len(), 10);
    g.assert_round_trips();
}

#[test]
fn a_circular_reference_is_an_error_in_those_cells_and_nowhere_else() {
    let mut g = invoice();
    g.input("subtotal", "total", "line");
    let Err(Error::Cycle(members)) = g.get::<ValueCell>("total") else { panic!("expected a cycle") };
    g.assert_round_trips();
    assert!(members.iter().any(|(e, _)| *e == g.id("subtotal")) && members.iter().any(|(e, _)| *e == g.id("total")), "{members:?}");
    assert_eq!(amount(&g, "gears"), 50.0, "cells outside it are fine");
    assert!(g.get::<ValueCell>("tax").is_err(), "those reading into it inherit the error");

    g.unlink("subtotal", "total", LABEL);
    assert_eq!(amount(&g, "total"), 96.0, "and it heals when the link goes");
}

#[test]
fn a_scan_enumerates_by_component_and_notices_new_rows() {
    let mut g = N::with(sheet);
    g.value("a", num(1.0)).value("b", num(2.0));
    g.formula("note", "sum(with.value)");
    // Its own value is one of those it scans, so a real column total lives in another cell.
    assert!(matches!(g.get::<ValueCell>("note"), Err(Error::Cycle(_))), "reading its own column is circular");

    let mut g = N::with(sheet);
    g.value("a", num(1.0)).value("b", num(2.0));
    g.formula_on("foot", "column_total", "sum(with.value)");
    assert_eq!(cell(&g, "foot", "column_total"), json!(3.0));
    g.value("c", num(4.0));
    assert_eq!(cell(&g, "foot", "column_total"), json!(7.0), "a new row is noticed");
}

/// One of the workspace's own cells, as JSON.
fn cell(g: &N, name: &str, cell: &str) -> Value {
    g.g.get_value(g.id(name), cell).unwrap().unwrap_or_else(|| panic!("{name} has no {cell}"))
}

// ---------- cells Rust can't type ----------

#[test]
fn cells_rust_cant_type_are_kept_and_a_formula_can_compute_with_them() {
    let mut g = invoice();
    let (widgets, gears) = (g.id("widgets"), g.id("gears"));
    g.g.set_value(widgets, "supplier", json!({ "name": "Acme", "lead_days": 12 }));
    let edge = g.g.link(widgets, gears, "ships_with");
    g.g.set_value(edge, "crate", json!(3));
    assert_eq!(cell(&g, "widgets", "supplier")["name"], "Acme");
    assert_eq!(g.g.get_value(gears, "crate").unwrap(), None, "an edge's cell is on the edge");
    assert_eq!(g.g.get_value(edge, "crate").unwrap(), Some(json!(3)));
    assert_eq!(amount(&g, "total"), 96.0, "ignored by everything that doesn't ask for it");

    // What a plugin's rule did, a formula does: it reads cells and edges of any component by name.
    g.formula_on("widgets", "lead_days", "self.supplier.lead_days + count(out.ships_with)");
    g.formula_on("widgets", "lead_value", "self.value");
    assert_eq!(cell(&g, "widgets", "lead_days"), json!(13.0));
    assert_eq!(cell(&g, "widgets", "lead_value"), json!(30.0), "typed cells are readable by name");

    // It's memoised like anything else, and its reads are tracked.
    g.take_ran();
    cell(&g, "widgets", "lead_days");
    assert!(g.ran().is_empty());
    g.g.set_value(widgets, "supplier", json!({ "name": "Acme", "lead_days": 20 }));
    assert_eq!(cell(&g, "widgets", "lead_days"), json!(21.0));
}

// ---------- the same sheet, with decisions held open ----------

fn trip() -> N {
    let mut g = N::with(plan::install);
    for (decision, options) in [("travel", [("train", 80.0), ("plane", 120.0)]), ("stay", [("hostel", 60.0), ("hotel", 150.0)])] {
        g.add(decision, DECISION);
        g.formula(decision, "pick(in.alternative)");
        for (option, price) in options {
            g.value(option, num(price)).link(option, decision, ALTERNATIVE);
        }
    }
    g.formula("trip", "travel + stay").input("trip", "travel", "travel").input("trip", "stay", "stay");
    g
}

fn costs(g: &N) -> recollect_graph::worlds::Cond<f64> {
    g.get::<ValueCell>("trip").unwrap().unwrap().0.0.map(|v| if let Val::Num(n) = v { *n } else { panic!("{v:?}") })
}

#[test]
fn a_budget_with_open_decisions_is_the_same_sheet() {
    let mut g = trip();
    let trip = costs(&g);
    assert_eq!((trip.min(), trip.max()), (140.0, 270.0));
    assert!((trip.expected() - 205.0).abs() < 1e-9);
    assert_eq!(trip.cases().len(), 4);
    let over_200 = trip.holds(|c| *c > 200.0);
    assert_eq!(g.show(&over_200), "stay=hotel", "exactly what puts you over budget");
    g.assert_round_trips();

    // Editing one price reruns only that decision and the total.
    g.take_ran();
    g.value("train", num(90.0));
    let trip = costs(&g);
    assert_eq!(g.ran_values(), vec!["travel", "trip"]);
    assert_eq!(trip.min(), 150.0);
}

#[test]
fn an_option_in_a_budget_can_carry_what_any_option_can() {
    use plan::Need;
    let mut g = trip();
    g.add("book the train", TASK).link("book the train", "train", PART_OF);
    assert_eq!(g.show(&g.get::<Need>("book the train").unwrap().unwrap().0), "travel=train", "it's the same kind of option as in a plan");
    assert_eq!(costs(&g).min(), 140.0, "and the price doesn't mind");
    assert_eq!(json!(g.get::<ValueCell>("train").unwrap().unwrap()), json!(80.0), "and it's still plain in a file");
}
