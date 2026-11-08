//! Toy 1: the maths you'd normally use a spreadsheet for. Each cell is either a typed-in number or
//! a formula over what it has labelled.

mod support;

use recollect_graph::{Error, Registry, Runtime, component, rule};
use recollect_systems::expr::Val;
use recollect_systems::formulas::{Label, ValueCell, install};
use recollect_systems::plan::{self, Alternative, Decision};
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
        g.set(&q, num(qty)).set(&p, num(price));
        g.formula(line, "quantity * price").input(line, &q, "quantity").input(line, &p, "price");
        g.input("subtotal", line, "line");
    }
    g.formula("subtotal", "sum(line)");
    g.set("tax_rate", num(0.2));
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

    g.set("widgets.price", num(11.0));
    assert_eq!(amount(&g, "total"), 99.6);
    let cells = g.ran_cells();
    let mut values: Vec<&String> = cells.iter().filter(|c| c.starts_with("value(")).collect();
    values.sort();
    assert_eq!(values, vec!["value(subtotal)", "value(tax)", "value(total)", "value(widgets)"], "gears doesn't run");
    assert!(cells.iter().filter(|c| c.starts_with("value_type(")).count() <= 1, "and no formula's type has to be worked out again: {cells:?}");

    g.set("widgets.price", num(11.0));
    amount(&g, "total");
    assert!(g.ran().is_empty(), "the same value isn't a change");
}

#[test]
fn a_recomputed_value_that_comes_out_the_same_stops_there() {
    // Swap two factors so a line's value is unchanged: nothing after it should run.
    let mut g = invoice();
    amount(&g, "total");
    g.take_ran();
    g.set("widgets.qty", num(6.0)).set("widgets.price", num(5.0));
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
    g.set("subtotal", num(1000.0));
    assert_eq!(amount(&g, "total"), 1200.0, "typed over, as in a spreadsheet");
    assert!(g.rule_of(g.id("subtotal"), "value").is_none());
    g.formula("subtotal", "sum(line)");
    assert_eq!(amount(&g, "total"), 96.0, "and back");
    assert_eq!(g.rule_of(g.id("subtotal"), "value").unwrap().1.outgoing, vec!["label"], "a rule says what it follows");
}

#[test]
fn fibonacci_is_linear_because_shared_cells_run_once() {
    let mut g = N::with(sheet);
    g.set("f0", num(0.0)).set("f1", num(1.0));
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
    g.set("y0", num(1000.0)).set("rate", num(0.05));
    for i in 1..=10 {
        let id = format!("y{i}");
        g.formula(&id, "last * (1 + rate)").input(&id, &format!("y{}", i - 1), "last").input(&id, "rate", "rate");
    }
    assert!((amount(&g, "y10") - 1628.894626777442).abs() < 1e-6);
    g.take_ran();
    g.set("rate", num(0.05));
    amount(&g, "y10");
    assert!(g.ran().is_empty());
    g.set("rate", num(0.1));
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

    g.unlink::<Label>("subtotal", "total");
    assert_eq!(amount(&g, "total"), 96.0, "and it heals when the link goes");
}

/// For checking what the core lets any rule do, with no formulas in the way.
#[derive(Copy)]
#[component]
struct Amount(f64);

#[component]
struct Addend;

#[test]
fn a_rule_can_only_read_what_it_declared() {
    let mut g = N::new();
    g.set("a", Amount(1.0)).set("b", Amount(2.0));
    let a = g.id("a");
    g.set("peek", rule("peek").compute(move |cx| Ok(cx.value::<Amount>(a)?)));
    assert!(matches!(g.get::<Amount>("peek"), Err(Error::Undeclared { .. })), "no declared link reaches a");
    g.set("links", rule("links").compute(|cx| Ok(Amount(cx.incoming::<Addend>()?.len() as f64))));
    assert!(matches!(g.get::<Amount>("links"), Err(Error::Undeclared { .. })), "and it didn't declare these links");
}

#[test]
fn a_rule_can_apply_to_a_whole_column() {
    // Every node that isn't typed in gets the rule, and it can opt out with no value at all.
    #[derive(Copy)]
    #[component]
    struct Doubled(f64);
    let mut g = N::new();
    g.set("a", Amount(2.0));
    g.e("note");
    g.set_default(rule("double").optional(|cx| Ok(cx.own::<Amount>()?.map(|a| Doubled(a.0 * 2.0)))));
    assert_eq!(g.get::<Doubled>("a").unwrap(), Some(Doubled(4.0)));
    assert_eq!(g.get::<Doubled>("note").unwrap(), None, "no Amount, so no Doubled");
    g.set("note", Amount(5.0));
    assert_eq!(g.get::<Doubled>("note").unwrap(), Some(Doubled(10.0)));
}

#[test]
fn a_scan_enumerates_by_type_and_notices_new_rows() {
    let mut g = N::new();
    g.set("a", Amount(1.0)).set("b", Amount(2.0));
    g.set("note", rule("total").scans::<Amount>().compute(|cx| {
        let rows = cx.with::<Amount>()?;
        Ok(Amount(rows.iter().filter(|(e, _)| *e != cx.id()).map(|(_, a)| a.0).sum()))
    }));
    // The rule's own cell holds Amount too, so a real column total lives in another component.
    assert!(matches!(g.get::<Amount>("note"), Err(Error::Cycle(_))), "reading its own column is circular");

    #[derive(Copy)]
    #[component]
    struct ColumnTotal(f64);
    let mut g = N::new();
    g.set("a", Amount(1.0)).set("b", Amount(2.0));
    g.set("foot", rule("total").scans::<Amount>().compute(|cx| Ok(ColumnTotal(cx.with::<Amount>()?.iter().map(|(_, a)| a.0).sum()))));
    assert_eq!(g.get::<ColumnTotal>("foot").unwrap(), Some(ColumnTotal(3.0)));
    g.set("c", Amount(4.0));
    assert_eq!(g.get::<ColumnTotal>("foot").unwrap(), Some(ColumnTotal(7.0)), "a new row is noticed");
}

// ---------- components Rust can't type ----------

#[test]
fn runtime_components_are_kept_and_a_runtime_rule_can_compute_with_them() {
    let mut g = invoice();
    let (widgets, gears) = (g.id("widgets"), g.id("gears"));
    g.g.set_runtime(widgets, "supplier", json!({ "name": "Acme", "lead_days": 12 }));
    g.g.link_runtime(widgets, gears, Runtime::new("ships_with", json!({ "crate": 3 })));
    assert_eq!(g.g.get_runtime(widgets, "supplier").unwrap().unwrap().value["name"], "Acme");
    assert_eq!(amount(&g, "total"), 96.0, "ignored by everything that doesn't ask for it");

    // Standing in for a TypeScript function: only strings and JSON cross this line.
    let lead = rule("lead").outgoing_named("ships_with").compute::<Value>(|cx| {
        let own = cx.get_runtime(cx.id(), "supplier")?.map_or(0, |s| s.value["lead_days"].as_i64().unwrap_or(0));
        let linked = cx.outgoing_runtime("ships_with")?.len() as i64;
        let amount = cx.get_runtime(cx.id(), "value")?.unwrap().value;
        Ok(json!({ "days": own + linked, "value": amount }))
    });
    g.g.set_runtime(widgets, "lead", lead);
    assert_eq!(g.g.get_runtime(widgets, "lead").unwrap().unwrap().value, json!({ "days": 13, "value": 30.0 }), "typed cells are readable by name");

    // It's memoised like anything else, and its reads are tracked.
    g.take_ran();
    g.g.get_runtime(widgets, "lead").unwrap();
    assert!(g.ran().is_empty());
    g.g.set_runtime(widgets, "supplier", json!({ "name": "Acme", "lead_days": 20 }));
    assert_eq!(g.g.get_runtime(widgets, "lead").unwrap().unwrap().value["days"], 21);
}

// ---------- the same sheet, with decisions held open ----------

fn trip() -> N {
    let mut g = N::with(plan::install);
    for (decision, options) in [("travel", [("train", 80.0), ("plane", 120.0)]), ("stay", [("hostel", 60.0), ("hotel", 150.0)])] {
        g.set(decision, Decision::default());
        g.formula(decision, "pick(in.alternative)");
        for (option, price) in options {
            g.set(option, num(price)).link(option, decision, Alternative::default());
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
    g.set("train", num(90.0));
    let trip = costs(&g);
    assert_eq!(g.ran_values(), vec!["travel", "trip"]);
    assert_eq!(trip.min(), 150.0);
}

#[test]
fn an_option_in_a_budget_can_carry_what_any_option_can() {
    use plan::{Need, PartOf, Task};
    let mut g = trip();
    g.set("book the train", Task {}).link("book the train", "train", PartOf);
    assert_eq!(g.show(&g.get::<Need>("book the train").unwrap().unwrap().0), "travel=train", "it's the same kind of option as in a plan");
    assert_eq!(costs(&g).min(), 140.0, "and the price doesn't mind");
    assert_eq!(json!(g.get::<ValueCell>("train").unwrap().unwrap()), json!(80.0), "and it's still plain in a file");
}
