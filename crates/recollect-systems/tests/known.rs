//! What's known and what's demanded: `evidence` and `requires`, as `docs/distributions.md` says. With
//! nothing a matter of chance they leave the same worlds. With something that is, evidence rules out
//! where it's false and never a choice, and a requirement rules out the choices that don't meet it
//! surely enough.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use recollect_graph::chance::{Law, Reader, Share, Term, phi};
use recollect_graph::worlds::{Bool, Space};
use recollect_graph::{CellType, CommitError, Entity, Log, Op, Registry};
use recollect_systems::expr::Val;
use recollect_systems::formulas::{AtLeast, EVIDENCE, OPTIONS, Options, REQUIRES, VALUE, ValueCell, install, known_draws, truth, worlds_left};
use support::N;

fn sheet(reg: &mut Registry) {
    install(reg);
}

fn options(g: &mut N, name: &str, options: &[&str]) {
    g.add(name, OPTIONS).set(name, Options(options.iter().map(|s| s.to_string()).collect()));
}

/// The worlds left, with these choices pinned.
fn left(g: &N, world: &[(&str, &str)]) -> Bool {
    let picks = Space::global().picks(&world.iter().map(|(name, option)| (name.to_string(), option.to_string())).collect::<BTreeMap<_, _>>());
    worlds_left(g, &picks, &Space::global().always(), &mut Reader::given(known_draws(g)))
}

fn value(g: &N, name: &str) -> ValueCell {
    g.get::<ValueCell>(name).unwrap_or_else(|e| panic!("{name}: {e:?}")).unwrap()
}

/// A choice that's yours between two routes, and rain that's a matter of chance: one time in four.
/// The first route is only fine when it's dry.
fn routes() -> N {
    let mut g = N::with(sheet);
    options(&mut g, "route", &["over the hill", "round the coast"]);
    g.formula("rain", "bernoulli(0.25)");
    g.formula("fine", r#"if(route.chosen.value = "over the hill", !rain, true)"#).input("fine", "route", "route").input("fine", "rain", "rain");
    g
}

fn rain(g: &N) -> String {
    format!("~{}/value#0", g.id("rain"))
}

#[test]
fn with_nothing_a_matter_of_chance_each_leaves_the_worlds_where_its_true() {
    for component in [REQUIRES, EVIDENCE] {
        let mut g = N::with(sheet);
        options(&mut g, "x", &["1", "2", "3"]);
        options(&mut g, "y", &["1", "2", "3"]);
        g.formula("more", "x.chosen.value > y.chosen.value").input("more", "x", "x").input("more", "y", "y").add("more", component);
        assert!(left(&g, &[]) == truth(&value(&g, "more")), "{component}");
        assert_eq!(left(&g, &[]).count_with(&[]), 3, "{component}");
    }
}

#[test]
fn evidence_rules_out_where_its_false_and_never_a_choice() {
    let mut g = routes();
    g.add("fine", EVIDENCE);
    let route = Space::global().uniform(&g.id("route").to_string(), &["over the hill", "round the coast"]);
    let worlds = left(&g, &[]);
    assert!(worlds == truth(&value(&g, "fine")));
    assert_eq!(route.possible(&worlds), ["over the hill", "round the coast"], "both routes are still yours to take");
    // What it says is about the rain: knowing the hill was fine, it was dry.
    let hill = &worlds & &route.is("over the hill");
    let wet = truth(&value(&g, "rain"));
    assert!((&hill & &wet).is_never() && !(&worlds & &wet).is_never());
}

#[test]
fn a_requirement_rules_out_the_choices_that_dont_meet_it_surely_enough() {
    let mut g = routes();
    g.add("fine", REQUIRES);
    let route = Space::global().uniform(&g.id("route").to_string(), &["over the hill", "round the coast"]);
    // Always, unless it says: the hill is only fine three times in four, so it's out, rain or shine.
    let worlds = left(&g, &[]);
    assert!(worlds == route.is("round the coast"), "{worlds:?}");
    assert!(!(&worlds & &truth(&value(&g, "rain"))).is_never(), "and nothing is said about the rain");

    g.set("fine", AtLeast(0.7));
    assert!(left(&g, &[]).is_always(), "three times in four is sure enough for 0.7");
    g.set("fine", AtLeast(0.8));
    assert!(left(&g, &[]) == route.is("round the coast"), "and not for 0.8");

    // Finding out about the rain is something known, so the hill is judged knowing it.
    g.set("fine", AtLeast(1.0));
    let rain = rain(&g);
    assert!(left(&g, &[(&rain, "false")]).is_always(), "dry, either route is fine");
    assert!(left(&g, &[(&rain, "true")]) == route.is("round the coast"), "wet, only the coast is");
}

#[test]
fn a_requirement_is_judged_given_the_evidence() {
    let mut g = routes();
    g.add("fine", REQUIRES);
    // It's known to be dry, so the hill is always fine.
    g.formula("dry", "!rain").input("dry", "rain", "rain").add("dry", EVIDENCE);
    let worlds = left(&g, &[]);
    assert!(worlds == !&truth(&value(&g, "rain")), "only the rain is ruled out: {worlds:?}");
}

#[test]
fn a_requirement_on_an_uncertain_number_is_met_as_often_as_its_law_says() {
    let mut g = N::with(sheet);
    options(&mut g, "supplier", &["day rate", "fixed"]);
    g.formula("cost", r#"if(supplier.chosen.value = "day rate", normal(3000, 500), 4200)"#).input("cost", "supplier", "supplier");
    g.formula("in budget", "cost < 4000").input("in budget", "cost", "cost").add("in budget", REQUIRES);
    let supplier = Space::global().uniform(&g.id("supplier").to_string(), &["day rate", "fixed"]);
    // The day rate is under budget unless it's two spreads over: 97.7% of the time. The fixed price never is.
    assert!((phi(2.0) - 0.977_249_868).abs() < 1e-9);
    assert!(left(&g, &[]).is_never(), "nothing is always in budget");
    g.set("in budget", AtLeast(0.95));
    assert!(left(&g, &[]) == supplier.is("day rate"));
    g.set("in budget", AtLeast(0.98));
    assert!(left(&g, &[]).is_never());
}

#[test]
fn evidence_about_an_uncertain_number_leaves_only_the_draws_where_it_holds() {
    let mut g = N::with(sheet);
    g.formula("x", "normal(0, 1)").formula("other", "normal(5, 1)");
    g.formula("above", "x > 0").input("above", "x", "x").add("above", EVIDENCE);
    g.formula("far", "x > 1").input("far", "x", "x");
    assert!(left(&g, &[]).is_always(), "it says nothing about any choice");
    let mut reader = Reader::given(known_draws(&g));
    let rand = |name: &str| match value(&g, name).0.0.cases()[0].1.clone() {
        Val::Rand(r) => r,
        other => panic!("{name} is {other:?}"),
    };
    // Half a bell curve: its mean is √(2/π), and it was read from the half of the draws that are left.
    let x = reader.summary(&[Share { weight: 1.0, value: Term::Rand(rand("x")) }]);
    assert!(!x.exact && x.draws == 2048 && x.low > 0.0 && (x.mean - (2.0 / std::f64::consts::PI).sqrt()).abs() < 3.0 * x.error, "{x:?}");
    // Over one spread, given it's over none: twice what it was.
    let far = reader.chance(&[Share { weight: 1.0, value: Term::Rand(rand("far")) }]);
    assert!(!far.exact && far.draws == 2048 && (far.chance - 2.0 * (1.0 - phi(1.0))).abs() < 3.0 * far.error, "{far:?}");
    // What shares nothing with it is what it was, exactly.
    let other = reader.summary(&[Share { weight: 1.0, value: Term::Rand(rand("other")) }]);
    assert!(other.exact && other.law.as_deref() == Some("normal(5, 1)"), "{other:?}");
    assert_eq!(reader.law(&rand("other")), Some(Law::Normal { mean: 5.0, sd: 1.0 }));

    // And a requirement is judged given it: under one spread, given over none, is 68% of the time.
    g.formula("near", "x < 1").input("near", "x", "x").add("near", REQUIRES).set("near", AtLeast(0.6));
    assert!(left(&g, &[]).is_always());
    g.set("near", AtLeast(0.75));
    assert!(left(&g, &[]).is_never());
}

#[test]
fn how_a_requirement_was_judged_is_said() {
    let mut reg = Registry::new();
    install(&mut reg);
    let mut log = Log::new(Arc::new(reg));
    let (route, rain, fine, cost, cap, mixed) = (Entity::new(), Entity::new(), Entity::new(), Entity::new(), Entity::new(), Entity::new());
    let label = |from: Entity, to: Entity, name: &str| [Op::link(from, to, "label"), Op::Set { on: recollect_graph::On::Edge { from, to }, cell: "called".to_string(), to: Some(recollect_graph::Stored::Value(serde_json::json!(name))) }];
    let mut ops: Vec<Op> = [route, rain, fine, cost, cap, mixed].into_iter().flat_map(|id| [Op::Spawn { id }, Op::add(id, VALUE)]).collect();
    ops.extend([Op::add(route, OPTIONS), Op::set(route, Options(vec!["over the hill".to_string(), "round the coast".to_string()]))]);
    ops.push(Op::set_formula(rain, ValueCell::NAME, "bernoulli(0.25)"));
    ops.push(Op::set_formula(fine, ValueCell::NAME, r#"if(route.chosen.value = "over the hill", !rain, true)"#));
    ops.extend(label(fine, route, "route"));
    ops.extend(label(fine, rain, "rain"));
    ops.push(Op::add(fine, REQUIRES));
    ops.push(Op::set_formula(cost, ValueCell::NAME, "normal(3000, 500)"));
    ops.push(Op::set_formula(cap, ValueCell::NAME, "cost < 4000"));
    ops.extend(label(cap, cost, "cost"));
    ops.push(Op::add(cap, REQUIRES));
    log.commit("t", "a route, a cost, and what's required of each", ops).unwrap();

    let advice = log.graph().registry().all_advice(log.graph());
    assert_eq!(advice.iter().filter(|a| a.on == fine).count(), 1, "said once, and nothing is too close to call: {advice:?}");
    let said = |on: Entity, rule: &str| advice.iter().find(|a| a.on == on && a.rule == rule).map(|a| a.detail.clone());
    let before = said(fine, "requirement_judged_before").expect("it depends on the rain");
    assert!(before.contains(&rain.to_string()) && !before.contains("No choice"), "{before}");
    // Nothing of yours bears on the cost, so requiring it be under 4,000 for certain can't be met.
    let cant = said(cap, "requirement_judged_before").expect("it depends on how the cost falls");
    assert!(cant.contains("how its uncertain numbers fall") && cant.contains("No choice makes it sure enough") && cant.contains("evidence"), "{cant}");

    // How sure is above nothing and at most certain.
    assert!(matches!(log.commit("t", "", vec![Op::set(cap, AtLeast(1.5))]), Err(CommitError::Refused(_))));
    log.commit("t", "", vec![Op::set(cap, AtLeast(0.9))]).unwrap();
    let advice = log.graph().registry().all_advice(log.graph());
    assert!(advice.iter().any(|a| a.on == cap && a.rule == "requirement_judged_before" && !a.detail.contains("No choice")));
    assert!(!advice.iter().any(|a| a.rule == "requirement_too_close_to_call"), "a law says how likely exactly, so there's nothing to be close: {advice:?}");

    // Read from draws, and within what those could be out by of how sure it has to be, it says so.
    let (a, b, both) = (Entity::new(), Entity::new(), Entity::new());
    let mut ops: Vec<Op> = [a, b, both].into_iter().flat_map(|id| [Op::Spawn { id }, Op::add(id, VALUE)]).collect();
    ops.extend([Op::set_formula(a, ValueCell::NAME, "normal(0, 1)"), Op::set_formula(b, ValueCell::NAME, "normal(0, 1)"), Op::set_formula(both, ValueCell::NAME, "a * b < 0")]);
    ops.extend(label(both, a, "a"));
    ops.extend(label(both, b, "b"));
    ops.extend([Op::add(both, REQUIRES), Op::set(both, AtLeast(0.5))]);
    log.commit("t", "half the time, as near as draws can say", ops).unwrap();
    let advice = log.graph().registry().all_advice(log.graph());
    assert!(advice.iter().any(|a| a.on == both && a.rule == "requirement_too_close_to_call"), "{advice:?}");

    // Known in some worlds as a matter of draws and in others not isn't worked out, and says so.
    let ops = vec![Op::set_formula(mixed, ValueCell::NAME, r#"if(route.chosen.value = "over the hill", cost < 4000, true)"#), Op::link(mixed, route, "label"), Op::link(mixed, cost, "label")];
    log.commit("t", "", ops).unwrap();
    log.commit("t", "", [label(mixed, route, "route"), label(mixed, cost, "cost")].concat()).unwrap();
    let Err(CommitError::Refused(broken)) = log.commit("t", "", vec![Op::add(mixed, EVIDENCE)]) else { panic!("it should be refused") };
    assert!(broken.iter().any(|v| v.rule == "evidence_partly_a_matter_of_draws"), "{broken:?}");
}
