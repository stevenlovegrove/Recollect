//! What comes after evidence and requirements: `chance` and `mean` in a cell, numbers read from draws
//! saying so, choices made knowing how something turns out, and what's found out by pinning reading the
//! same as settling it. See `docs/distributions.md`.

mod support;

use std::collections::BTreeMap;

use recollect_graph::chance::Reader;
use recollect_graph::worlds::{Bool, Space};
use recollect_graph::{CellType, Error, Graph, Registry};
use recollect_systems::expr::{Estimate, Val};
use recollect_systems::formulas::{AtLeast, Chosen, EVIDENCE, FOUND, Found, OPTIONS, Options, REQUIRES, ValueCell, install, known_draws, worlds_left};
use recollect_systems::plan::{ALTERNATIVE, DECISION, Estimate as Days, KNOWING, TASK, UNCERTAIN, Weight};
use support::N;

fn sheet(reg: &mut Registry) {
    install(reg);
}

fn planned() -> N {
    N::systems(&["plan"])
}

fn value(g: &Graph, e: recollect_graph::Entity) -> recollect_graph::worlds::Cond<Val> {
    g.get::<ValueCell>(e).unwrap_or_else(|err| panic!("{err:?}")).unwrap().0.0
}

fn only(g: &N, name: &str) -> Val {
    let v = value(g, g.id(name));
    let cases = v.cases();
    assert_eq!(cases.len(), 1, "{name} is {v:?}");
    cases[0].1.clone()
}

fn estimate(v: &Val) -> Estimate {
    match v {
        Val::Estimate(e) => e.clone(),
        other => panic!("{other:?} isn't an estimate"),
    }
}

fn options(g: &mut N, name: &str, options: &[&str]) {
    g.add(name, OPTIONS).set(name, Options(options.iter().map(|s| s.to_string()).collect()));
}

/// The worlds where a choice lands on the option with this name.
fn lands(g: &N, choice: &str, option: &str) -> Bool {
    let chosen = g.get::<Chosen>(choice).unwrap().unwrap();
    chosen.0.0.holds(|v| matches!(v, Val::Ref(e) if recollect_systems::label_of(g, *e) == option))
}

/// The worlds every requirement and piece of evidence leaves.
fn left(g: &N) -> Bool {
    let picks = Space::global().picks(&BTreeMap::new());
    worlds_left(g, &picks, &Space::global().always(), &mut Reader::given(known_draws(g)))
}

// ---------- chance and mean ----------

#[test]
fn chance_and_mean_are_exact_where_a_law_says_and_estimates_where_draws_do() {
    let mut g = N::with(sheet);
    g.formula("x", "normal(3, 1)");
    g.formula("average", "mean(x)").input("average", "x", "x");
    g.formula("over", "chance(x > 4)").input("over", "x", "x");
    g.formula("product", "mean(x * x)").input("product", "x", "x");
    g.formula("plain", "mean(5)");
    assert_eq!((only(&g, "average"), only(&g, "plain")), (Val::Num(3.0), Val::Num(5.0)));
    let Val::Num(over) = only(&g, "over") else { panic!() };
    assert!((over - 0.158_655_253_931_457).abs() < 1e-12);
    // x² has no law: it's read from draws, and says so. Its mean is 3² + 1.
    let e = estimate(&only(&g, "product"));
    assert!(e.draws == recollect_graph::chance::DRAWS && e.error.is_some_and(|err| err > 0.0 && (e.value - 10.0).abs() < 4.0 * err), "{e:?}");
}

#[test]
fn an_estimate_says_so_through_what_follows_from_it() {
    let mut g = N::with(sheet);
    g.formula("x", "normal(3, 1)");
    g.formula("squared", "mean(x * x)").input("squared", "x", "x");
    for (name, text) in [("moved", "s + 1"), ("doubled", "s * 2"), ("rooted", "sqrt(s)"), ("far", "s > 100"), ("near", "s > 10"), ("same", "s = 10")] {
        g.formula(name, text).input(name, "squared", "s");
    }
    let s = estimate(&only(&g, "squared"));
    let err = s.error.unwrap();
    let moved = estimate(&only(&g, "moved"));
    assert!((moved.value - (s.value + 1.0)).abs() < 1e-12 && moved.error == Some(err));
    assert_eq!(estimate(&only(&g, "doubled")).error, Some(2.0 * err));
    assert_eq!(estimate(&only(&g, "rooted")).error, None, "it still says it's an estimate, and no longer how far out");
    assert_eq!(only(&g, "far"), Val::Bool(false), "well clear of what it could be out by, it can be called");
    assert_eq!((only(&g, "near"), only(&g, "same")), (Val::Undefined, Val::Undefined), "too close to call, so it isn't");
    // And a cell that has to hold a number can't hold one read from draws.
    let mut g = planned();
    g.formula("x", "normal(3, 1)");
    g.add("t", TASK).formula_on("t", Days::NAME, "mean(x * x)").input("t", "x", "x");
    let Err(Error::Failed(why)) = g.get::<Days>("t") else { panic!("it should say why") };
    assert!(why.contains("can't be something read from draws"), "{why}");
}

#[test]
fn chance_is_over_matters_of_chance_and_never_over_a_choice_of_yours() {
    let mut g = planned();
    options(&mut g, "route", &["over the hill", "round the coast"]);
    g.formula("rain", "bernoulli(0.25)");
    g.formula("fine", r#"if(route.chosen.value = "over the hill", !rain, true)"#).input("fine", "route", "route").input("fine", "rain", "rain");
    g.formula("odds", "chance(fine)").input("odds", "fine", "fine");
    // One for each route.
    let odds = value(&g, g.id("odds"));
    let at = |o: &str| odds.at(&BTreeMap::from([(g.id("route").to_string(), o.to_string())])).clone();
    assert_eq!((at("over the hill"), at("round the coast")), (Val::Num(0.75), Val::Num(1.0)));
    // An uncertain decision is a matter of chance, so it's averaged over, as believed.
    g.add("weather", DECISION).add("weather", UNCERTAIN);
    for (o, w) in [("sun", 3.0), ("storm", 1.0)] {
        g.e(o);
        g.link(o, "weather", ALTERNATIVE).set_on_edge(o, "weather", Weight(recollect_graph::worlds::Worldly::plain(Some(w))));
    }
    g.formula("calm", "chose(w, sun)").input("calm", "weather", "w").input("calm", "sun", "sun");
    g.formula("calm odds", "chance(calm)").input("calm odds", "calm", "calm");
    assert_eq!(only(&g, "calm odds"), Val::Num(0.75));
    // Known to be dry, the hill is always fine.
    g.formula("dry", "!rain").input("dry", "rain", "rain").add("dry", EVIDENCE);
    assert_eq!(value(&g, g.id("odds")).len(), 1);
    assert_eq!(only(&g, "odds"), Val::Num(1.0));
}

#[test]
fn what_a_view_finds_out_reads_the_same_as_settling_it() {
    let mut g = planned();
    g.add("weather", DECISION).add("weather", UNCERTAIN);
    for (o, w) in [("sun", 3.0), ("storm", 1.0)] {
        g.e(o);
        g.link(o, "weather", ALTERNATIVE).set_on_edge(o, "weather", Weight(recollect_graph::worlds::Worldly::plain(Some(w))));
    }
    g.formula("calm", "chose(w, sun)").input("calm", "weather", "w").input("calm", "sun", "sun");
    g.formula("calm odds", "chance(calm)").input("calm odds", "calm", "calm");
    g.formula("twice", "odds * 2").input("twice", "calm odds", "odds");
    for (o, want) in [("sun", 2.0), ("storm", 0.0)] {
        let mut found = g.g.fork();
        let f = found.spawn();
        found.add(f, FOUND).set(f, Found(BTreeMap::from([(g.id("weather").to_string(), g.id(o).to_string())])));
        let mut settled = g.g.fork();
        settled.set(g.id("weather"), Chosen::on(g.id(o)));
        let read = |h: &Graph| value(h, g.id("twice"));
        assert_eq!(read(&found), read(&settled), "{o}");
        assert_eq!(read(&found).cases()[0].1, &Val::Num(want));
    }
    assert_eq!(only(&g, "twice"), Val::Num(1.5), "and the graph itself is as it was");
}

// ---------- knowing ----------

/// Whether you take an umbrella, and rain one time in ten: you stay dry if you take it, or if it doesn't rain.
fn umbrella() -> N {
    let mut g = planned();
    g.add("umbrella", DECISION);
    options(&mut g, "umbrella", &["take", "leave"]);
    g.formula("rain", "bernoulli(0.1)");
    g.formula("dry", r#"umbrella.chosen.value = "take" || !rain"#).input("dry", "umbrella", "umbrella").input("dry", "rain", "rain");
    g.add("dry", REQUIRES).set("dry", AtLeast(0.95));
    g
}

#[test]
fn a_choice_made_knowing_something_is_judged_for_each_way_it_could_turn_out() {
    let mut g = umbrella();
    let rain = value(&g, g.id("rain")).holds(|v| *v == Val::Bool(true));
    // Not knowing, leaving it is dry nine times in ten, which isn't enough.
    let worlds = left(&g);
    assert!(worlds == lands(&g, "umbrella", "take"), "{worlds:?}");
    // Knowing whether it'll rain, leaving it is fine on a dry day, and only then.
    g.link("umbrella", "rain", KNOWING);
    let worlds = left(&g);
    assert!(worlds == (&lands(&g, "umbrella", "take") | &!&rain), "{worlds:?}");
    let advice = g.registry().all_advice(&g);
    assert!(advice.iter().any(|a| a.rule == "requirement_judged_knowing" && a.detail.contains("umbrella made knowing rain")), "{advice:?}");
}

#[test]
fn an_earlier_choice_is_allowed_only_where_a_later_one_can_then_meet_it() {
    let mut g = planned();
    g.add("route", DECISION);
    options(&mut g, "route", &["hill", "coast"]);
    g.add("shelter", DECISION);
    options(&mut g, "shelter", &["wait", "go"]);
    g.formula("rain", "bernoulli(0.5)");
    // Over the hill in the rain you have to wait to stay fine, and waiting makes you late.
    let ok = r#"(route.chosen.value = "coast" || !rain || shelter.chosen.value = "wait") && (shelter.chosen.value = "go" || route.chosen.value = "coast")"#;
    g.formula("ok", ok).input("ok", "route", "route").input("ok", "shelter", "shelter").input("ok", "rain", "rain");
    g.add("ok", REQUIRES);
    g.link("shelter", "rain", KNOWING);
    // On a dry day the hill would do, but you choose the route before you know: so only the coast.
    let worlds = left(&g);
    assert!(worlds == lands(&g, "route", "coast"), "{worlds:?}");
}

#[test]
fn what_cant_be_judged_rules_nothing_out_and_says_why() {
    // Two choices that know different things.
    let mut g = umbrella();
    g.add("coat", DECISION);
    options(&mut g, "coat", &["on", "off"]);
    g.formula("cold", "bernoulli(0.5)");
    g.formula("dry", r#"(umbrella.chosen.value = "take" || !rain) && (coat.chosen.value = "on" || !cold)"#).input("dry", "coat", "coat").input("dry", "cold", "cold");
    g.link("umbrella", "rain", KNOWING).link("coat", "cold", KNOWING);
    assert!(left(&g).is_always());
    let advice = g.registry().all_advice(&g);
    assert!(advice.iter().any(|a| a.rule == "requirement_not_judged" && a.detail.contains("know different things")), "{advice:?}");

    // A choice that knows an uncertain number.
    let mut g = umbrella();
    g.formula("rain", "normal(0, 1) > 1");
    g.formula("depth", "normal(0, 1)");
    g.link("umbrella", "depth", KNOWING);
    g.formula("dry", r#"umbrella.chosen.value = "take" || d < 1"#).input("dry", "depth", "d");
    assert!(left(&g).is_always());
    let advice = g.registry().all_advice(&g);
    assert!(advice.iter().any(|a| a.rule == "requirement_not_judged" && a.detail.contains("uncertain number")), "{advice:?}");
}
