//! Data in tables means what `docs/data.md` says: a row picked as one draw keeps its columns together,
//! and `predict` is exact inference about a straight line through the rows, its line and spread shared
//! by everything fitted to the same data.

mod support;

use recollect_graph::Error;
use recollect_graph::chance::{self, Rand, Reader, Share, Term};
use recollect_graph::worlds::{Cond, Worldly};
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

fn rand(v: &Val) -> Rand {
    match v {
        Val::Rand(r) => *r,
        other => panic!("{other:?} isn't uncertain"),
    }
}

fn error(g: &N, name: &str) -> String {
    match g.get::<ValueCell>(name) {
        Err(Error::Failed(why)) => why,
        other => panic!("{name} should have failed, and is {other:?}"),
    }
}

/// A cell of the workspace's own on a row, in its one world.
fn cell(g: &N, row: &str, cell: &str) -> Val {
    let w: Worldly<Val> = serde_json::from_value(g.get_value(g.id(row), cell).unwrap().unwrap_or_else(|| panic!("{row} has no {cell}"))).unwrap();
    let cases = w.0.cases();
    assert_eq!(cases.len(), 1, "{row}.{cell} is {:?}", w.0);
    cases[0].1.clone()
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

// ---------- χ² ----------

#[test]
fn a_chi_square_draw_falls_where_the_tables_say() {
    // Two degrees of freedom is an exponential with a mean of 2, which has a formula to check against.
    for p in [0.001, 0.1, 0.5, 0.9, 0.999] {
        let want = -2.0 * (1.0 - p as f64).ln();
        let got = 2.0 * chance::gamma_quantile(1.0, p);
        assert!((got - want).abs() < 1e-9 * want.max(1.0), "χ²₂ at {p}: {got} and {want}");
    }
    // As scipy has them: one degree at 95%, ten at 5%, a hundred at 99%, and half a degree, a shape
    // under one, at 30%.
    for (k, p, want) in [(1.0, 0.95, 3.841_458_820_694_120_5), (10.0, 0.05, 3.940_299_136_119_060_5), (100.0, 0.99, 135.806_723_171_026_76), (0.5, 0.3, 0.010_982_604_948_550_948)] {
        let got = 2.0 * chance::gamma_quantile(k / 2.0, p);
        assert!((got - want).abs() < 1e-9 * want, "χ²{k} at {p}: {got} and {want}");
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

// ---------- predict ----------

const YS: [f64; 8] = [10.0, 12.0, 9.0, 14.0, 11.0, 13.0, 10.5, 12.5];

fn mean_sd(r: &Rand) -> (f64, f64, f64) {
    let s = Reader::new().summary(&[Share { weight: 1.0, value: Term::Rand(*r) }]);
    assert!(!s.exact, "a prediction is read from draws, and says so");
    (s.mean, s.sd, s.error)
}

#[test]
fn a_prediction_with_nothing_to_go_on_is_the_textbooks() {
    let mut g = N::with(install);
    let rows: Vec<[f64; 1]> = YS.iter().map(|y| [*y]).collect();
    let rows: Vec<&[f64]> = rows.iter().map(|r| r.as_slice()).collect();
    table(&mut g, "T", &["y"], &rows);
    g.formula("next", "predict(r in T.rows: r.y)");
    let next = rand(&only(&g, "next"));
    let n = YS.len() as f64;
    let mean = YS.iter().sum::<f64>() / n;
    let s = (YS.iter().map(|y| (y - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
    // A Student t with n - 1 degrees of freedom, about the mean, scaled by s√(1 + 1/n).
    let dof = n - 1.0;
    let want_sd = s * (1.0 + 1.0 / n).sqrt() * (dof / (dof - 2.0)).sqrt();
    let (got_mean, got_sd, err) = mean_sd(&next);
    assert!((got_mean - mean).abs() < 4.0 * err, "middle {got_mean}, wanted {mean} ± {err}");
    assert!((got_sd - want_sd).abs() < 0.05 * want_sd, "spread {got_sd}, wanted {want_sd}");

    // And draw by draw it's exactly what it says it is.
    let names = next.draws();
    let find = |end: &str| names.iter().find(|d| d.ends_with(end)).unwrap_or_else(|| panic!("no draw ending {end} in {names:?}")).clone();
    let (spread, line, own) = (find("#spread"), find("#line0"), find("value#0"));
    let us = [(spread.clone(), 0.3), (line.clone(), 0.8), (own.clone(), 0.1)];
    let at = |name: &str| us.iter().find(|(n, _)| n == name).map(|(_, u)| *u).unwrap();
    let sigma = s * (dof / (2.0 * chance::gamma_quantile(dof / 2.0, 0.3))).sqrt();
    let want = mean + sigma * chance::probit(0.8) / n.sqrt() + sigma * chance::probit(0.1);
    let got = next.at(&at).unwrap();
    assert!((got - want).abs() < 1e-9, "{got} and {want}");
}

/// Rows on a line, `y = 3 + 2x`, give or take.
fn line(g: &mut N) {
    let xs = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let wobble = [0.3, -0.2, 0.1, -0.4, 0.2, 0.0, -0.1, 0.3];
    let rows: Vec<[f64; 2]> = xs.iter().zip(wobble).map(|(x, w)| [*x, 3.0 + 2.0 * x + w]).collect();
    let rows: Vec<&[f64]> = rows.iter().map(|r| r.as_slice()).collect();
    table(g, "T", &["x", "y"], &rows);
    g.e("New");
    g.formula_on("New", "guess", "predict(r in T.rows: r.y, r.x)");
    g.formula_on("New", "alone", "predict(r in T.rows: r.y)");
    for (key, x) in [("New[1]", 4.5), ("New[2]", 20.0), ("New[3]", 21.0)] {
        g.e(key);
        g.formula_on(key, "x", &x.to_string());
    }
}

#[test]
fn a_prediction_is_the_least_squares_line_at_the_rows_own_x_and_wider_further_out() {
    let mut g = N::with(install);
    line(&mut g);
    let xs: Vec<f64> = (1..=8).map(f64::from).collect();
    let ys: Vec<f64> = xs.iter().zip([0.3, -0.2, 0.1, -0.4, 0.2, 0.0, -0.1, 0.3]).map(|(x, w)| 3.0 + 2.0 * x + w).collect();
    let (mx, my) = (xs.iter().sum::<f64>() / 8.0, ys.iter().sum::<f64>() / 8.0);
    let slope = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum::<f64>() / xs.iter().map(|x| (x - mx).powi(2)).sum::<f64>();
    let height = my - slope * mx;
    let near = rand(&cell(&g, "New[1]", "guess"));
    let far = rand(&cell(&g, "New[2]", "guess"));
    let (m1, sd1, e1) = mean_sd(&near);
    let (m2, sd2, e2) = mean_sd(&far);
    assert!((m1 - (height + slope * 4.5)).abs() < 4.0 * e1, "{m1}");
    assert!((m2 - (height + slope * 20.0)).abs() < 4.0 * e2, "{m2}");
    assert!(sd2 > 2.0 * sd1, "far from the data it's wider: {sd1} near and {sd2} far");
    let (_, alone, _) = mean_sd(&rand(&cell(&g, "New[1]", "alone")));
    assert!(alone > 10.0 * sd1, "knowing x is much sharper: {alone} without and {sd1} with");
}

#[test]
fn predictions_from_the_same_data_share_its_line_and_spread() {
    let mut g = N::with(install);
    line(&mut g);
    // Two far from the data, where how unsure the line is matters most.
    let (a, b) = (rand(&cell(&g, "New[2]", "guess")), rand(&cell(&g, "New[3]", "guess")));
    let shared: Vec<String> = a.draws().into_iter().filter(|d| b.draws().contains(d)).collect();
    assert_eq!(shared.len(), 3, "the spread and the line's two parts, and not each one's own noise: {shared:?}");
    // So how far apart they are is much narrower than if they were unrelated: mostly their own noise.
    let gap = chance::bin(chance::Op::Sub, &Term::Rand(b), &Term::Rand(a));
    let Term::Rand(gap) = gap else { panic!("{gap:?}") };
    let (_, sa, _) = mean_sd(&a);
    let (_, sb, _) = mean_sd(&b);
    let (_, sg, _) = mean_sd(&gap);
    assert!(sg < 0.75 * (sa * sa + sb * sb).sqrt(), "{sg} against {}", (sa * sa + sb * sb).sqrt());
}

#[test]
fn predict_says_what_it_can_t_fit() {
    let mut g = N::with(install);
    table(&mut g, "T", &["x", "y"], &[&[1.0, 2.0], &[2.0, 4.0], &[3.0, 5.0]]);
    g.formula_on("T[2]", "x", "undefined");
    g.formula("missing", "predict(r in T.rows: r.y, r.x)");
    assert!(error(&g, "missing").contains("T[2] is missing one"), "{}", error(&g, "missing"));
    g.formula_on("T[2]", "x", "normal(2, 1)");
    assert!(error(&g, "missing").contains("T[2]'s is uncertain"), "{}", error(&g, "missing"));
    g.formula_on("T[2]", "x", "1");
    g.formula_on("T[3]", "x", "1");
    assert!(error(&g, "missing").contains("no one line"), "{}", error(&g, "missing"));
    g.formula("few", "predict(r in T.rows: r.y, r.x, r.x * r.x)");
    assert!(error(&g, "few").contains("3 rows for a height and 2 slopes"), "{}", error(&g, "few"));
}
