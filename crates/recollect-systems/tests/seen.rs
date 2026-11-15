//! What's seen, with how likely it was, as `docs/inference.md` says: `y ~ normal(…)` as evidence. Over
//! short lists it's exact, as Bayes' rule by hand. Over draws, each draw counts as much as it makes what
//! was seen likely, and what's read says how many draws that's worth.

mod support;

use recollect_graph::{CellType, Error};
use recollect_graph::chance::{Reader, Share, Term};
use recollect_systems::expr::{Estimate, Val, parse};
use recollect_systems::formulas::{EVIDENCE, REQUIRES, ValueCell, install, known_draws};
use support::N;

fn only(g: &N, name: &str) -> Val {
    let v = g.get::<ValueCell>(name).unwrap_or_else(|e| panic!("{name}: {e:?}")).unwrap().0.0;
    let cases = v.cases();
    assert_eq!(cases.len(), 1, "{name} is {v:?}");
    cases[0].1.clone()
}

fn number(g: &N, name: &str) -> f64 {
    match only(g, name) {
        Val::Num(n) => n,
        other => panic!("{name} is {other:?}, not a number worked out exactly"),
    }
}

fn estimate(g: &N, name: &str) -> Estimate {
    match only(g, name) {
        Val::Estimate(e) => e,
        other => panic!("{name} is {other:?}, not read from draws"),
    }
}

fn error(g: &N, name: &str) -> String {
    match g.get::<ValueCell>(name) {
        Err(Error::Failed(why)) => why,
        other => panic!("{name} should have failed, and is {other:?}"),
    }
}

// ---------- over short lists, exactly ----------

/// A coin whose bias is one of a quarter, a half or three quarters, each as likely.
fn coin(g: &mut N) {
    g.formula("k", "integers(1, 3)");
    g.formula("Bias", "mean(k / 4)").input("Bias", "k", "k");
}

#[test]
fn a_toss_seen_weighs_each_bias_by_how_likely_it_made_it() {
    let mut g = N::with(install);
    coin(&mut g);
    assert!((number(&g, "Bias") - 0.5).abs() < 1e-12);
    g.formula("Heads", "true ~ bernoulli(k / 4)").input("Heads", "k", "k").add("Heads", EVIDENCE);
    // Bayes by hand: each bias times how likely it made heads, 1:2:3, so the bias is (1 + 4 + 9) / 24.
    assert!((number(&g, "Bias") - 14.0 / 24.0).abs() < 1e-12, "{}", number(&g, "Bias"));
    // A tails as well: 1·3 : 2·2 : 3·1, which is a half again.
    g.formula("Then tails", "false ~ bernoulli(k / 4)").input("Then tails", "k", "k").add("Then tails", EVIDENCE);
    assert!((number(&g, "Bias") - 0.5).abs() < 1e-12, "{}", number(&g, "Bias"));
}

#[test]
fn both_seen_with_and_is_the_same_as_each_on_its_own() {
    let mut g = N::with(install);
    coin(&mut g);
    g.formula("Both", "true ~ bernoulli(k / 4) && false ~ bernoulli(k / 4)").input("Both", "k", "k").add("Both", EVIDENCE);
    assert!((number(&g, "Bias") - 0.5).abs() < 1e-12);
    // All of a group seen, as a table's rows would be.
    let mut g = N::with(install);
    coin(&mut g);
    g.e("Toss");
    for (i, h) in ["true", "true", "true", "false"].iter().enumerate() {
        let k = format!("Toss[{}]", i + 1);
        g.e(&k);
        g.formula_on(&k, "heads", h);
    }
    g.formula("All", "all(t in Toss.rows: t.heads ~ bernoulli(k / 4))").input("All", "k", "k").add("All", EVIDENCE);
    // Three heads and a tail: 1·1·1·3 : 2·2·2·2 : 3·3·3·1 = 3 : 16 : 27.
    let want = (3.0 * 0.25 + 16.0 * 0.5 + 27.0 * 0.75) / 46.0;
    assert!((number(&g, "Bias") - want).abs() < 1e-12, "{} and {want}", number(&g, "Bias"));
}

#[test]
fn a_reading_from_a_bell_curve_weighs_each_middle_by_its_density() {
    let mut g = N::with(install);
    g.formula("mu", "integers(1, 3)");
    g.formula("Middle", "mean(mu)").input("Middle", "mu", "mu");
    g.formula("Read", "2.2 ~ normal(mu, 1)").input("Read", "mu", "mu").add("Read", EVIDENCE);
    let w: Vec<f64> = [1.0f64, 2.0, 3.0].iter().map(|m| (-0.5 * (2.2 - m) * (2.2 - m)).exp()).collect();
    let want = (w[0] + 2.0 * w[1] + 3.0 * w[2]) / w.iter().sum::<f64>();
    assert!((number(&g, "Middle") - want).abs() < 1e-12, "{} and {want}", number(&g, "Middle"));
}

#[test]
fn what_couldnt_have_come_from_a_world_rules_it_out() {
    let mut g = N::with(install);
    g.formula("sides", "integers(4, 6)");
    g.formula("Average", "mean(sides)").input("Average", "sides", "sides");
    // A 5 can't come from a four-sided die, and is likelier from a five-sided one than a six.
    g.formula("Rolled", "5 ~ integers(1, sides)").input("Rolled", "sides", "sides").add("Rolled", EVIDENCE);
    let want = (5.0 / 5.0 + 6.0 / 6.0) / (1.0 / 5.0 + 1.0 / 6.0);
    assert!((number(&g, "Average") - want).abs() < 1e-12, "{} and {want}", number(&g, "Average"));
}

#[test]
fn not_yet_known_it_changes_nothing() {
    let mut g = N::with(install);
    coin(&mut g);
    g.formula("Heads", "true ~ bernoulli(k / 4)").input("Heads", "k", "k");
    assert!((number(&g, "Bias") - 0.5).abs() < 1e-12);
}

// ---------- over draws, weighted ----------

#[test]
fn a_reading_of_a_bell_curve_is_weighed_draw_by_draw_and_says_what_its_worth() {
    let mut g = N::with(install);
    g.formula("m", "normal(2, 0.5)");
    g.formula("Middle", "mean(m)").input("Middle", "m", "m");
    g.formula("Seen", "3 ~ normal(m, 1)").input("Seen", "m", "m").add("Seen", EVIDENCE);
    // A bell curve seen through a bell curve: precision 4 + 1, so the middle is (2·4 + 3·1)/5 and the
    // spread √(1/5).
    let e = estimate(&g, "Middle");
    assert!(e.draws < 4096 && e.draws > 2000, "weighted, so worth fewer than all of them: {}", e.draws);
    assert!((e.value - 2.2).abs() < 4.0 * e.error.unwrap(), "{e:?}");
    let known = known_draws(&g);
    let Val::Rand(m) = only(&g, "m") else { panic!() };
    let s = Reader::given(known).summary(&[Share { weight: 1.0, value: Term::Rand(m) }]);
    assert!(s.given.is_some() && !s.exact && s.draws == e.draws, "{s:?}");
    assert!((s.sd - 0.2f64.sqrt()).abs() < 0.02, "{}", s.sd);
}

/// A line through the first `rows` runners, with residuals a bell curve of spread 1.
fn line(rows: usize) -> N {
    const RUN: [(f64, f64); 40] = [(25.7, 53.0), (19.4, 40.8), (18.8, 40.0), (26.2, 55.0), (26.2, 54.2), (32.2, 69.7), (20.3, 44.1), (29.5, 62.7), (25.2, 54.0), (23.1, 50.8), (30.4, 63.2), (26.8, 55.6), (23.3, 50.2), (24.7, 52.0), (21.2, 42.8), (23.5, 51.8), (24.6, 51.6), (22.4, 45.9), (16.8, 35.5), (26.3, 55.9), (25.5, 51.5), (21.3, 44.2), (24.6, 51.0), (25.8, 53.3), (22.6, 47.7), (19.8, 41.3), (27.2, 57.2), (24.8, 52.6), (31.2, 63.6), (19.7, 40.7), (29.6, 62.1), (23.0, 48.3), (23.9, 52.2), (24.6, 51.7), (21.4, 44.2), (25.7, 52.5), (20.4, 42.4), (24.7, 51.4), (21.7, 45.5), (17.7, 36.3)];
    let mut g = N::with(install);
    g.formula("m", "normal(2, 0.5)");
    g.formula("c", "normal(0, 5)");
    g.e("T");
    for (i, (x, y)) in RUN.iter().take(rows).enumerate() {
        let k = format!("T[{}]", i + 1);
        g.e(&k);
        g.formula_on(&k, "x", &x.to_string()).formula_on(&k, "y", &y.to_string());
    }
    g.formula("T", "all(r in rows: r.y ~ normal(m * r.x + c, 1))").input("T", "m", "m").input("T", "c", "c").add("T", EVIDENCE);
    g.formula("Slope", "mean(m)").input("Slope", "m", "m");
    g
}

/// The slope as read given what's known, with how it was got.
fn slope(g: &N) -> recollect_graph::chance::Summary {
    let Val::Rand(m) = only(g, "m") else { panic!("the slope is uncertain") };
    Reader::given(known_draws(g)).summary(&[Share { weight: 1.0, value: Term::Rand(m) }])
}

#[test]
fn a_line_through_the_runners_is_the_bell_curves_answer_however_many_rows() {
    use recollect_graph::chance::Given;
    // Against a fine grid over the same prior, rows, and spread of 1: the slope's middle and spread.
    for (rows, mean, sd) in [(1, 2.054, 0.184), (5, 2.004, 0.109), (40, 2.099, 0.043)] {
        let s = slope(&line(rows));
        assert_eq!(s.given, Some(Given::Moved), "{rows} rows leave too few draws' worth weighted, so they're moved");
        assert!(s.draws > 2000, "{rows}: worth {} draws", s.draws);
        assert!((s.mean - mean).abs() < 4.0 * sd / (s.draws as f64).sqrt(), "{rows}: {} and {mean}", s.mean);
        assert!((s.sd - sd).abs() < 0.1 * sd, "{rows}: spread {} and {sd}", s.sd);
    }
}

#[test]
fn where_weighing_is_enough_the_draws_stay_where_they_are() {
    use recollect_graph::chance::Given;
    let mut g = N::with(install);
    g.formula("m", "normal(2, 0.5)");
    g.formula("Seen", "3 ~ normal(m, 1)").input("Seen", "m", "m").add("Seen", EVIDENCE);
    assert_eq!(slope(&g).given, Some(Given::Weighted));
}

#[test]
fn two_humps_far_apart_are_both_kept() {
    // Something seen near 1 from x², with x anywhere about 0 beforehand: x is about 1 or about -1.
    let mut g = N::with(install);
    g.formula("m", "normal(0, 3)");
    g.formula("Seen", "1 ~ normal(m * m, 0.1)").input("Seen", "m", "m").add("Seen", EVIDENCE);
    g.formula("Positive", "chance(m > 0)").input("Positive", "m", "m");
    g.formula("Size", "mean(abs(m))").input("Size", "m", "m");
    let s = slope(&g);
    assert_eq!(s.given, Some(recollect_graph::chance::Given::Moved));
    let positive = estimate(&g, "Positive");
    assert!((positive.value - 0.5).abs() < 0.1, "both humps, about evenly: {positive:?}");
    assert!((estimate(&g, "Size").value - 1.0).abs() < 0.05, "{:?}", estimate(&g, "Size"));
}

#[test]
fn moving_is_the_same_every_time() {
    let g = line(10);
    let (a, b) = (slope(&g), slope(&g));
    assert_eq!(a, b);
}

// ---------- what it is, and isn't ----------

#[test]
fn the_right_of_a_tilde_is_a_distribution() {
    assert!(parse("y ~ normal(0, 1)").is_ok());
    assert!(parse("y ~ 3").unwrap_err().contains("says what it's drawn from"));
    assert!(parse("y ~ max(1, 2)").unwrap_err().contains("says what it's drawn from"));
    let mut g = N::with(install);
    g.formula("Few", "1 ~ normal(0)");
    assert!(error(&g, "Few").contains("normal takes 2"), "{}", error(&g, "Few"));
    g.formula("Text", r#"1 ~ normal("a", 1)"#);
    assert!(error(&g, "Text").contains("needs numbers"), "{}", error(&g, "Text"));
}

#[test]
fn what_was_seen_goes_together_only_with_and() {
    let mut g = N::with(install);
    g.formula("m", "normal(0, 1)");
    for (name, f) in [("Or", "1 ~ normal(m, 1) || 2 ~ normal(m, 1)"), ("Not", "!(1 ~ normal(m, 1))"), ("If", "if(1 ~ normal(m, 1), 1, 2)")] {
        g.formula(name, f).input(name, "m", "m");
        assert!(g.get::<ValueCell>(name).is_err(), "{name} should be refused");
    }
    // With a truth, it's seen where the truth holds.
    g.formula("Where", "m > 0 && 1 ~ normal(m, 1)").input("Where", "m", "m").add("Where", EVIDENCE);
    g.formula("Middle", "mean(m)").input("Middle", "m", "m");
    assert!(estimate(&g, "Middle").value > 0.0);
}

#[test]
fn a_requirement_cant_be_something_seen() {
    use recollect_graph::{CommitError, Entity, Log, Op, Registry};
    use recollect_systems::formulas::VALUE;
    let mut reg = Registry::new();
    install(&mut reg);
    let mut log = Log::new(std::sync::Arc::new(reg));
    let (m, seen) = (Entity::new(), Entity::new());
    let mut ops: Vec<Op> = [m, seen].into_iter().flat_map(|id| [Op::Spawn { id }, Op::add(id, VALUE)]).collect();
    ops.push(Op::set_formula(m, ValueCell::NAME, "normal(0, 1)"));
    ops.push(Op::set_formula(seen, ValueCell::NAME, "1 ~ normal(m, 1)"));
    ops.extend([Op::link(seen, m, "label"), Op::Set { on: recollect_graph::On::Edge { from: seen, to: m }, cell: "called".to_string(), to: Some(recollect_graph::Stored::Value(serde_json::json!("m"))) }]);
    log.commit("t", "", ops).unwrap();
    let Err(CommitError::Refused(why)) = log.commit("t", "", vec![Op::add(seen, REQUIRES)]) else { panic!("it should be refused") };
    assert!(format!("{why:?}").contains("evidence instead"), "{why:?}");
    log.commit("t", "", vec![Op::add(seen, EVIDENCE)]).unwrap();
}


