//! Data in tables means what `docs/data.md` says: a row picked as one draw keeps its columns together.

mod support;

use recollect_graph::Error;
use recollect_graph::worlds::Cond;
use recollect_systems::expr::Val;
use recollect_systems::formulas::{EVIDENCE, ValueCell, install};
use support::N;

fn value(g: &N, name: &str) -> Cond<Val> {
    g.get::<ValueCell>(name).unwrap_or_else(|e| panic!("{name}: {e:?}")).unwrap().0.0
}

fn only(g: &N, name: &str) -> Val {
    let v = value(g, name);
    let cases = v.cases();
    assert_eq!(cases.len(), 1, "{name} is {v:?}");
    cases[0].1.clone()
}

fn error(g: &N, name: &str) -> String {
    match g.get::<ValueCell>(name) {
        Err(Error::Failed(why)) => why,
        other => panic!("{name} should have failed, and is {other:?}"),
    }
}

/// A table of rows, each with these cells.
fn table(g: &mut N, name: &str, columns: &[&str], rows: &[&[f64]]) {
    g.e(name);
    for (i, row) in rows.iter().enumerate() {
        let key = format!("{name}[{}]", i + 1);
        g.e(&key);
        for (c, v) in columns.iter().zip(row.iter()) {
            g.formula_on(&key, c, &v.to_string());
        }
    }
}

// ---------- a row as one draw ----------

fn runners(g: &mut N) {
    table(g, "T", &["x", "y"], &[&[1.0, 2.0], &[2.0, 4.0], &[3.0, 6.0], &[4.0, 9.0]]);
    g.formula("pick", "any_row(T.rows)");
    g.formula("px", "pick.value.x").input("px", "pick", "pick");
    g.formula("py", "pick.value.y").input("py", "pick", "pick");
}

#[test]
fn a_picked_row_is_each_row_as_often_as_the_others() {
    let mut g = N::with(install);
    runners(&mut g);
    let px = value(&g, "px");
    assert_eq!(px.len(), 4);
    for (when, v) in px.cases() {
        assert!((when.prob() - 0.25).abs() < 1e-12, "{v:?} comes up {}", when.prob());
    }
}

#[test]
fn columns_read_through_one_pick_move_together() {
    let mut g = N::with(install);
    runners(&mut g);
    g.formula("gap", "y - 2 * x").input("gap", "py", "y").input("gap", "px", "x");
    // Each row's own gap, never one row's y against another's x.
    let mut gaps: Vec<f64> = value(&g, "gap").cases().iter().map(|(_, v)| if let Val::Num(n) = v { *n } else { panic!("{v:?}") }).collect();
    gaps.sort_by(f64::total_cmp);
    assert_eq!(gaps, vec![0.0, 1.0]);
    // Two picks are two rows, drawn apart.
    g.formula("pick2", "any_row(T.rows)");
    g.formula("two", "a.value.x - b.value.x").input("two", "pick", "a").input("two", "pick2", "b");
    assert_eq!(value(&g, "two").len(), 7, "differences from -3 to 3");
}

#[test]
fn evidence_about_one_column_leaves_the_rows_it_holds_for() {
    let mut g = N::with(install);
    runners(&mut g);
    g.formula("Average y", "mean(y)").input("Average y", "py", "y");
    assert_eq!(only(&g, "Average y"), Val::Num(5.25));
    g.formula("Quick", "x > 2").input("Quick", "px", "x").add("Quick", EVIDENCE);
    assert_eq!(only(&g, "Average y"), Val::Num(7.5), "only the rows with x over 2");
}

#[test]
fn a_pick_needs_rows() {
    let mut g = N::with(install);
    g.e("Empty");
    g.formula("pick", "any_row(Empty.rows)");
    assert!(error(&g, "pick").contains("has none"), "{}", error(&g, "pick"));
}
