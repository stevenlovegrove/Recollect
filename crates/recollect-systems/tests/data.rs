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
    g.formula("pick", "one_of(T.rows)");
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
    g.formula("pick2", "one_of(T.rows)");
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
    g.formula("pick", "one_of(Empty.rows)");
    assert!(error(&g, "pick").contains("has nothing"), "{}", error(&g, "pick"));
}

// ---------- a line fitted as it's written ----------

/// A line through the runners written as equations: a slope and a height with what's believed of them
/// beforehand, a residual on each row, and what's known about the residuals as evidence.
fn line_fit(rows: usize, within: f64) -> N {
    let mut g = N::with(install);
    g.formula("m", "normal(2, 0.5)");
    g.formula("c", "normal(0, 5)");
    g.e("T");
    for (i, (x, y)) in RUNNERS.iter().take(rows).enumerate() {
        let k = format!("T[{}]", i + 1);
        g.e(&k);
        g.formula_on(&k, "x", &x.to_string()).formula_on(&k, "y", &y.to_string());
    }
    g.formula_on("T", "residual", "self.y - (m * self.x + c)").input("T", "m", "m").input("T", "c", "c");
    g.formula("Fits", &format!("all(r in T.rows: abs(r.residual) < {within})")).add("Fits", EVIDENCE);
    g.formula("Slope", "mean(m)").input("Slope", "m", "m");
    g
}

const RUNNERS: [(f64, f64); 40] = [(25.7, 53.0), (19.4, 40.8), (18.8, 40.0), (26.2, 55.0), (26.2, 54.2), (32.2, 69.7), (20.3, 44.1), (29.5, 62.7), (25.2, 54.0), (23.1, 50.8), (30.4, 63.2), (26.8, 55.6), (23.3, 50.2), (24.7, 52.0), (21.2, 42.8), (23.5, 51.8), (24.6, 51.6), (22.4, 45.9), (16.8, 35.5), (26.3, 55.9), (25.5, 51.5), (21.3, 44.2), (24.6, 51.0), (25.8, 53.3), (22.6, 47.7), (19.8, 41.3), (27.2, 57.2), (24.8, 52.6), (31.2, 63.6), (19.7, 40.7), (29.6, 62.1), (23.0, 48.3), (23.9, 52.2), (24.6, 51.7), (21.4, 44.2), (25.7, 52.5), (20.4, 42.4), (24.7, 51.4), (21.7, 45.5), (17.7, 36.3)];

#[test]
fn a_line_fitted_as_evidence_about_residuals_keeps_the_draws_that_fit_and_says_how_few() {
    use recollect_graph::chance::Reader;
    // Every residual within 5 minutes: about one draw in eight fits, and the slope given that is what a
    // fine grid over the same prior and the same evidence says, 2.098 give or take 0.148.
    let g = line_fit(40, 5.0);
    let kept = Reader::given(recollect_systems::formulas::known_draws(&g)).draws_kept();
    assert!((450..550).contains(&kept), "{kept} draws fit");
    let Val::Estimate(slope) = only(&g, "Slope") else { panic!("read from the draws that fit, and says so") };
    assert_eq!(slope.draws, kept);
    assert!((slope.value - 2.098).abs() < 3.0 * slope.error.unwrap(), "{slope:?}");
    // Within 2 minutes, no line fits all forty: the grid finds none either. So there's nothing to say.
    let g = line_fit(40, 2.0);
    assert_eq!(Reader::given(recollect_systems::formulas::known_draws(&g)).draws_kept(), 0);
    assert_eq!(only(&g, "Slope"), Val::Undefined);
}


/// The same line, with a parallel table of residuals, each a draw of its own, and each row saying it's
/// the line plus its residual: exactly, or within `within`.
fn residual_table(rows: usize, within: Option<f64>) -> N {
    let mut g = N::with(install);
    g.formula("m", "normal(2, 0.5)");
    g.formula("c", "normal(0, 5)");
    g.e("T");
    g.e("E");
    for (i, (x, y)) in RUNNERS.iter().take(rows).enumerate() {
        let k = format!("T[{}]", i + 1);
        g.e(&k);
        g.formula_on(&k, "x", &x.to_string()).formula_on(&k, "y", &y.to_string());
        g.e(&format!("E[{}]", i + 1));
    }
    g.formula_on("E", "e", "normal(0, 1)");
    let each = match within {
        None => "r.y = m * r.x + c + E[r.row].e".to_string(),
        Some(w) => format!("abs(r.y - (m * r.x + c + E[r.row].e)) < {w}"),
    };
    g.formula("Fits", &format!("all(r in T.rows: {each})")).input("Fits", "m", "m").input("Fits", "c", "c").add("Fits", EVIDENCE);
    g.formula("Slope", "mean(m)").input("Slope", "m", "m");
    g
}

#[test]
fn a_residual_drawn_for_each_row_fits_exactly_nowhere_and_within_a_little_for_only_a_few_rows() {
    use recollect_graph::chance::Reader;
    let kept = |g: &N| Reader::given(recollect_systems::formulas::known_draws(g)).draws_kept();
    // A number that could be anything is never exactly another, so with even one row nothing's left.
    let g = residual_table(1, None);
    assert_eq!((kept(&g), only(&g, "Slope")), (0, Val::Undefined));
    // Within half a minute, one row leaves what a grid says it should, about 117 draws, and the slope
    // they say is the bell curve's.
    let g = residual_table(1, Some(0.5));
    assert!((90..150).contains(&kept(&g)), "{}", kept(&g));
    let Val::Estimate(slope) = only(&g, "Slope") else { panic!() };
    assert!((slope.value - 2.054).abs() < 3.0 * slope.error.unwrap(), "{slope:?}");
    // Each row keeps a few in ten of the draws the last one left: by five rows there's none.
    assert_eq!(kept(&residual_table(5, Some(0.5))), 0);
}
