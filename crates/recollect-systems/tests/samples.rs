//! The sample workspaces, as committed in `examples/`, still mean what the example problems say they
//! mean. Each is opened the way the app opens a workspace, from its files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use recollect_graph::worlds::{Bool, Cond, Space, Var};
use recollect_graph::{CellType, Entity, Graph, Log, Name};
use recollect_systems::expr::Val;
use recollect_systems::formulas::{REQUIRES, ValueCell, options_of, truth};
use recollect_systems::plan::{self, Need, Pressure};
use recollect_systems::samples::{SAMPLES, ops_of};
use recollect_systems::schedule::{Finish, Slack};
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

struct Opened {
    log: Log,
    ids: BTreeMap<String, Entity>,
}

impl Opened {
    fn g(&self) -> &Graph {
        self.log.graph()
    }
    fn id(&self, name: &str) -> Entity {
        *self.ids.get(name).unwrap_or_else(|| panic!("no node called {name}"))
    }
    fn get<C: CellType>(&self, name: &str) -> C {
        self.g().get::<C>(self.id(name)).unwrap().unwrap()
    }
    /// A value that's one number in every world.
    fn number(&self, name: &str) -> f64 {
        let c = self.get::<ValueCell>(name).0.0;
        let cases = c.cases();
        assert_eq!(cases.len(), 1, "{name}");
        if let Val::Num(n) = cases[0].1 { *n } else { panic!("{name} is {:?}", cases[0].1) }
    }
}

fn open(name: &str) -> Opened {
    let sample = SAMPLES.iter().find(|s| s.name == name).unwrap();
    let systems: Vec<String> = sample.systems.iter().map(|s| s.to_string()).collect();
    let built = recollect_systems::build(&systems).unwrap();
    let (g, problems) = Graph::load_dir(&root().join(name), built.registry.clone()).unwrap();
    assert_eq!(problems, vec![], "{name} loads with nothing to report");
    let (log, problems) = Log::from_saved(g.save(), built.registry);
    assert_eq!(problems, vec![], "{name} opens with nothing to report");
    let ids = log.graph().entities().filter_map(|e| Some((log.graph().get::<Name>(e).ok()??.0, e))).collect();
    Opened { log, ids }
}

/// The choices a puzzle's solutions range over, and its solutions by name.
fn solutions(o: &Opened, limit: usize) -> (Bool, Vec<Var>, Vec<BTreeMap<String, String>>) {
    let g = o.g();
    let puzzle = g.entities().find(|e| g.has(*e, REQUIRES)).unwrap();
    let valid = truth(&g.get::<ValueCell>(puzzle).unwrap().unwrap());
    let mut vars = Vec::new();
    for e in g.entities() {
        if let Some(options) = options_of(g, e) {
            let options: Vec<&str> = options.iter().map(String::as_str).collect();
            vars.push(Space::global().uniform(&e.to_string(), &options));
        }
    }
    let names = recollect_systems::labels(g);
    let solved = valid.solutions(&vars, limit).into_iter().map(|s| s.into_iter().map(|(k, v)| (names[&k].clone(), v)).collect()).collect();
    (valid, vars, solved)
}

#[test]
fn every_sample_is_there_and_is_what_its_generator_makes() {
    for s in SAMPLES {
        let o = open(s.name);
        // Handles are minted fresh each time, so compare what's made of them.
        let names: Vec<String> = s.systems.iter().map(|n| n.to_string()).collect();
        let mut fresh = Log::new(recollect_systems::build(&names).unwrap().registry);
        fresh.commit("t", "", ops_of(s)).unwrap();
        let shape = |g: &Graph| {
            let mut out: Vec<(Option<String>, Vec<String>, usize)> = g.save().nodes.into_iter().map(|n| (g.get::<Name>(n.id).ok().flatten().map(|n| n.0), n.components, n.links.len())).collect();
            out.sort();
            out
        };
        assert_eq!(shape(o.g()), shape(fresh.graph()), "{}", s.name);
        let files = std::fs::read_dir(root().join(s.name)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(files, ["nodes".to_string(), "recollect.toml".to_string()].into(), "{}: only the files a workspace has", s.name);
        assert!(o.log.violations().is_empty(), "{}: {:?}", s.name, o.log.violations());
        let text = std::fs::read_to_string(root().join(s.name).join("recollect.toml")).unwrap();
        assert_eq!(text, recollect_systems::samples::config_text(s), "{}", s.name);
    }
    let dirs = std::fs::read_dir(root()).unwrap().filter(|e| e.as_ref().unwrap().path().is_dir()).count();
    assert_eq!(dirs, SAMPLES.len(), "and nothing else is");
}

#[test]
fn the_invoice_adds_up_and_reruns_only_what_it_reaches() {
    let mut o = open("invoice");
    assert_eq!(o.number("Total"), 96.0);
    o.g().take_ran();
    let price = o.id("Widgets: price");
    o.log.commit("t", "dearer", vec![recollect_graph::Op::set(price, ValueCell::num(11.0))]).unwrap();
    assert_eq!(o.number("Total"), 99.6);
    assert_eq!(o.g().take_ran().iter().filter(|(_, c)| c == ValueCell::NAME).count(), 4, "Widgets, Subtotal, Tax and Total, and not Gears");
}

#[test]
fn the_trip_budget_is_exact_with_its_choices_open() {
    let o = open("trip-budget");
    let trip: Cond<f64> = o.get::<ValueCell>("Trip").0.0.map(|v| if let Val::Num(n) = v { *n } else { panic!("{v:?}") });
    assert_eq!((trip.min(), trip.max()), (140.0, 270.0));
    assert!((trip.expected() - 205.0).abs() < 1e-9);
    let over = trip.holds(|c| *c > 200.0);
    let names = recollect_systems::labels(o.g());
    let show = |b: &Bool| b.show_with(&|s| names.get(s).cloned().unwrap_or_else(|| s.to_string()));
    assert_eq!(show(&over), "Stay=Hotel");
    assert_eq!(show(&truth(&o.get::<ValueCell>("Over £200"))), "Stay=Hotel", "and the formula says the same");
}

#[test]
fn compound_interest_and_fibonacci_reach_their_known_values() {
    // A table's own column, worked out row by row from the one before.
    // Most rows aren't written, so they're found by where they are.
    let column = |o: &Opened, row: &str, c: &str| -> Value {
        let at = recollect_graph::table::row_of(row).unwrap();
        let e = o.g().row_at(&at.table, &at.key).unwrap_or_else(|| panic!("no row {row}"));
        o.g().get_value(e, c).unwrap().unwrap_or_else(|| panic!("{row} has no {c}"))
    };
    let o = open("compound-interest");
    // With the top-up in year 5 still open, the last year is one of two balances.
    let last: Vec<f64> = serde_json::from_value::<recollect_graph::worlds::Worldly<Val>>(column(&o, "Year[10]", "balance")).unwrap().0.cases().iter().map(|(_, v)| if let Val::Num(n) = v { *n } else { panic!("{v:?}") }).collect();
    let (lo, hi) = (last.iter().copied().fold(f64::MAX, f64::min), last.iter().copied().fold(f64::MIN, f64::max));
    assert!((lo - 1628.894626777442).abs() < 1e-6 && (hi - lo - 500.0 * 1.05f64.powi(5)).abs() < 1e-6, "{last:?}");
    let o = open("fibonacci");
    assert_eq!(column(&o, "F[80]", "f").as_f64().unwrap(), 23416728348467685.0);
    assert_eq!(o.g().take_ran().iter().filter(|(_, c)| c == "f").count(), 79);
    assert_eq!(o.number("The 80th"), 23416728348467685.0, "and a formula can ask for it");
}

#[test]
fn the_plan_samples_hold_their_decisions_open() {
    let o = open("handbook");
    let g = o.g();
    let names = recollect_systems::labels(g);
    let show = |b: &Bool| b.show_with(&|s| names.get(s).cloned().unwrap_or_else(|| s.to_string()));
    assert_eq!(show(&o.get::<Need>("Lay out the pages").0), "How do we publish it?∈{As a printed booklet,As a PDF}");
    assert!(o.get::<Need>("Agree who approves it").0.is_always());
    let end = o.get::<Finish>("How do we publish it?").0.0.plain();
    let at = |option: &str| *end.at(&BTreeMap::from([(o.id("How do we publish it?").to_string(), o.id(option).to_string())]));
    assert_eq!((at("As a printed booklet"), at("As a PDF"), at("As a website"), at("Something else")), (Some(5.0), Some(7.0), Some(4.0), None));
    assert!(matches!(plan::pressure(g, o.id("How do we publish it?")).unwrap(), Pressure::Free(_)));

    let o = open("critical-path");
    let slack = |n: &str| *o.get::<Slack>(n).0.0.plain().at(&BTreeMap::new());
    assert_eq!(slack("Wire the rooms"), Some(2.0));
    assert_eq!(slack("Frame the walls"), Some(0.0));
    assert_eq!(*o.get::<Finish>("Paint").0.0.plain().at(&BTreeMap::new()), Some(9.0));
}

#[test]
fn the_puzzles_have_their_known_answers() {
    let (valid, vars, _) = solutions(&open("australia"), 1);
    assert_eq!(valid.count(&vars), 18);
    let (valid, vars, _) = solutions(&open("eight-queens"), 1);
    assert_eq!(valid.count(&vars), 92);
    let (valid, vars, _) = solutions(&open("sudoku"), 1);
    assert_eq!(valid.count(&vars), 288);

    let (valid, vars, solved) = solutions(&open("knights-and-knaves"), 10);
    assert_eq!(valid.count(&vars), 1);
    assert_eq!((solved[0]["A"].as_str(), solved[0]["B"].as_str()), ("knave", "knight"));

    let (valid, vars, solved) = solutions(&open("zebra"), 1);
    assert_eq!(valid.count(&vars), 1);
    assert_eq!(solved[0]["Pet[Zebra]"], solved[0]["Nationality[Japanese]"]);
    assert_eq!(solved[0]["Drink[Water]"], solved[0]["Nationality[Norwegian]"]);
}

#[test]
fn a_drawing_is_an_ordinary_node_in_the_log() {
    use recollect_systems::notes::Drawing;
    for (sample, name) in [("sudoku", "Board with its boxes"), ("australia", "Map of Australia")] {
        let o = open(sample);
        let Drawing(source) = o.get::<Drawing>(name);
        assert!(source.contains("export function draw"), "{sample}");
        assert!(o.g().get::<ValueCell>(o.id(name)).unwrap().is_none(), "and it doesn't take part in the puzzle");
    }
    // It doesn't change what the puzzles mean either.
    let (valid, vars, _) = solutions(&open("sudoku"), 1);
    assert_eq!(valid.count(&vars), 288);
}

#[test]
fn the_launch_is_likely_within_a_fortnight_and_says_how_it_knows() {
    use recollect_graph::chance::{Num, Reader, Share, Term};
    use recollect_systems::plan::Estimate;
    let o = open("launch");
    let mut reader = Reader::new();
    let one = |v: &Val| match v {
        Val::Rand(r) => [Share { weight: 1.0, value: Term::Rand(*r) }],
        other => panic!("{other:?} isn't uncertain"),
    };

    // An estimate is the law it's written as, exactly.
    let design = o.get::<Estimate>("Design the pages").0.0;
    let Some(Num::Rand(design)) = design.cases()[0].1.clone() else { panic!("{design:?}") };
    let s = reader.summary(&[Share { weight: 1.0, value: Term::Rand(design) }]);
    assert!(s.exact && s.law.as_deref() == Some("triangular(3, 4, 8)") && s.mean == 5.0, "{s:?}");

    // The review is a day, or two to five, depending on a coin that comes up three times in ten.
    let review = o.get::<Estimate>("Review and fix").0.0;
    assert_eq!(review.len(), 2);
    let quick = review.cases().iter().find(|(_, d)| **d == Some(Num::Plain(1.0))).map(|(when, _)| when.prob()).expect("a day if it passes");
    assert!((quick - 0.7).abs() < 1e-12);

    // The end differs over the coin and by chance. With no changes asked for it's the build's chain or the
    // copy, then a day: at least 3 + 4 + 1, and on average about 5 + 7⅓ + 1.
    let end = o.get::<ValueCell>("Days until launch").0.0;
    assert_eq!(end.len(), 2);
    let changes = o.id("The review asks for changes");
    let passes = end.at(&BTreeMap::from([(format!("~{changes}/value#0"), "false".to_string())]));
    let s = reader.summary(&one(passes));
    assert!(!s.exact && s.draws == 4096 && s.least == Some(8.0) && (s.mean - 13.33).abs() < 0.1, "{s:?}");

    // Whether that's within a fortnight is a chance, read from the same draws.
    let soon = o.get::<ValueCell>("Done within a fortnight").0.0;
    let passes = soon.at(&BTreeMap::from([(format!("~{changes}/value#0"), "false".to_string())]));
    let c = reader.chance(&one(passes));
    assert!(!c.exact && c.chance > 0.55 && c.chance < 0.7, "{c:?}");
    let fails = soon.at(&BTreeMap::from([(format!("~{changes}/value#0"), "true".to_string())]));
    assert!(reader.chance(&one(fails)).chance < c.chance, "and it's less likely if changes are asked for");
}

/// What a sample's node comes to, where that's one uncertain thing.
fn uncertain(o: &Opened, name: &str) -> recollect_graph::chance::Rand {
    let value = o.get::<ValueCell>(name).0.0;
    match value.cases().as_slice() {
        [(_, Val::Rand(r))] => *r,
        other => panic!("{name} isn't one uncertain thing: {other:?}"),
    }
}

#[test]
fn savings_drawn_each_year_average_out_and_one_rate_compounds() {
    use recollect_graph::chance::{Law, Reader, Share, Term, phi};
    let o = open("savings");
    let mut reader = Reader::new();
    let lognormal = |law: Option<Law>| match law {
        Some(Law::Lognormal { mu, sigma }) => (mu, sigma),
        other => panic!("{other:?} isn't a lognormal"),
    };
    // Ten years' growth of about 5%: the same middle either way, and a tenth as much spread squared.
    let (mu, sigma) = lognormal(reader.law(&uncertain(&o, "After 10 years")));
    assert!((mu - (1000f64.ln() + 0.5)).abs() < 1e-9 && (sigma - 0.1 * 10f64.sqrt()).abs() < 1e-9, "{mu} {sigma}");
    let (steady_mu, steady_sigma) = lognormal(reader.law(&uncertain(&o, "Steady after 10 years")));
    assert!((steady_mu - mu).abs() < 1e-9 && (steady_sigma - 1.0).abs() < 1e-9, "{steady_mu} {steady_sigma}");

    let mut sure = |name: &str| {
        let c = reader.chance(&[Share { weight: 1.0, value: Term::Rand(uncertain(&o, name)) }]);
        assert!(c.exact, "{name}: {c:?}");
        c.chance
    };
    let over = |sigma: f64| 1.0 - phi((2000f64.ln() - mu) / sigma);
    assert!((sure("Doubled in ten years") - over(sigma)).abs() < 1e-9 && (sure("Steady doubled in ten years") - over(1.0)).abs() < 1e-9);
    assert!(sure("Doubled in ten years") < sure("Steady doubled in ten years"), "more spread, more chance of the extremes");
    assert!((sure("Year by year comes out ahead") - 0.5).abs() < 1e-9);
}

#[test]
fn each_suppliers_cost_is_held_as_what_it_is() {
    use recollect_graph::chance::{Law, Reader, Share, Term};
    let o = open("supplier");
    let which = o.id("Which supplier?").to_string();
    let at = |option: &str| BTreeMap::from([(which.clone(), o.id(option).to_string())]);
    let cost = o.get::<ValueCell>("Cost").0.0;
    assert_eq!(cost.len(), 3);
    let mut reader = Reader::new();
    let mut summary = |option: &str| {
        let Val::Rand(r) = cost.at(&at(option)) else { panic!("{option}'s cost is uncertain, if only for the shipping") };
        (reader.law(r), reader.summary(&[Share { weight: 1.0, value: Term::Rand(*r) }]))
    };
    // A plain quote and uncertain shipping is the shipping, moved along.
    let (law, fixed) = summary("Fixed price");
    assert_eq!(law, Some(Law::Uniform { from: 4300.0, to: 4500.0 }));
    assert!(fixed.exact && fixed.mean == 4400.0);
    // The others are sums of unlike things, which have no law, so they're read from draws.
    let (law, day) = summary("Day rate");
    assert!(law.is_none() && !day.exact && (day.mean - (12_800.0 / 3.0 + 200.0)).abs() < 3.0 * day.error, "{day:?}");
    let (law, extras) = summary("Fixed with extras");
    assert!(law.is_none() && !extras.exact && extras.least == Some(3700.0) && (extras.mean - 4200.0).abs() < 3.0 * extras.error, "{extras:?}");

    let within = o.get::<ValueCell>("Within budget").0.0;
    assert_eq!(within.at(&at("Fixed price")), &Val::Bool(true), "at most 4,200 and 300");
    let Val::Rand(r) = within.at(&at("Day rate")) else { panic!("a chance") };
    let c = reader.chance(&[Share { weight: 1.0, value: Term::Rand(*r) }]);
    assert!(!c.exact && c.chance > 0.5 && c.chance < 0.8, "{c:?}");
}

#[test]
fn the_shelf_probably_fits_and_how_likely_is_exact() {
    use recollect_graph::chance::{Law, Reader, Share, Term, phi};
    let o = open("shelf");
    let mut reader = Reader::new();
    // Nine times in ten between 1,198 and 1,204 is a spread of 3 over 1.645.
    let sd = (4.0 + (3.0f64 / 1.6448536269514722).powi(2)).sqrt();
    let Some(Law::Normal { mean, sd: found }) = reader.law(&uncertain(&o, "Clearance")) else { panic!("clearance is a bell curve") };
    assert!((mean - 4.0).abs() < 1e-9 && (found - sd).abs() < 1e-9, "{mean} {found}");
    let mut sure = |name: &str| {
        let c = reader.chance(&[Share { weight: 1.0, value: Term::Rand(uncertain(&o, name)) }]);
        assert!(c.exact, "{name}: {c:?}");
        c.chance
    };
    assert!((sure("It fits") - phi(4.0 / sd)).abs() < 1e-9);
    assert!((sure("A snug fit") - (phi(-1.0 / sd) - phi(-4.0 / sd))).abs() < 1e-9);
    assert_eq!(o.get::<ValueCell>("Clearance less itself").0.0.cases()[0].1, &Val::Num(0.0));
}

#[test]
fn two_dice_have_their_known_chances() {
    let o = open("two-dice");
    let total = o.get::<ValueCell>("Total").0.0;
    assert_eq!(total.len(), 11);
    for (when, v) in total.cases() {
        let Val::Num(n) = v else { panic!("{v:?}") };
        assert!((when.prob() - (6.0 - (n - 7.0).abs()) / 36.0).abs() < 1e-12, "{n}");
    }
    let chance = |name: &str| truth(&o.get::<ValueCell>(name)).prob();
    assert!((chance("A seven") - 1.0 / 6.0).abs() < 1e-12 && (chance("Doubles") - 1.0 / 6.0).abs() < 1e-12 && (chance("A six somewhere") - 11.0 / 36.0).abs() < 1e-12);
    let higher = o.get::<ValueCell>("The higher of the two").0.0;
    assert!((higher.holds(|v| *v == Val::Num(6.0)).prob() - 11.0 / 36.0).abs() < 1e-12 && (higher.holds(|v| *v == Val::Num(1.0)).prob() - 1.0 / 36.0).abs() < 1e-12);
    // Finding out the first die leaves a seven as likely as it was, and makes a total of two impossible.
    let first = format!("~{}/value#0", o.id("First die"));
    let picks = Space::global().picks(&BTreeMap::from([(first, "6".to_string())]));
    assert!((truth(&o.get::<ValueCell>("A seven")).given(&picks).prob() - 1.0 / 6.0).abs() < 1e-12);
    assert_eq!(total.given(&picks).len(), 6);
}

#[test]
fn monty_hall_is_judged_knowing_which_door_he_opened() {
    use recollect_graph::Op;
    use recollect_systems::formulas::{AtLeast, Chosen, known_draws, worlds_left};
    let mut o = open("monty-hall");
    // How likely you are to win, for each way your choices go: never averaged over those.
    let odds = o.get::<ValueCell>("Chance of winning").0.0;
    for (_, v) in odds.cases() {
        let Val::Num(p) = v else { panic!("{v:?}") };
        assert!((p - 1.0 / 3.0).abs() < 1e-12 || (p - 2.0 / 3.0).abs() < 1e-12, "{p}");
    }
    let win = o.id("You win the car");
    let switch = |o: &Opened| o.get::<Chosen>("Stick or switch?").0.0.holds(|v| matches!(v, Val::Ref(e) if recollect_systems::label_of(o.g(), *e) == "Switch"));
    let left = |o: &Opened| worlds_left(o.g(), &Space::global().picks(&BTreeMap::new()), &Space::global().always(), &mut recollect_graph::chance::Reader::given(known_draws(o.g())));
    o.log.commit("t", "require winning", vec![Op::add(win, REQUIRES), Op::set(win, AtLeast(0.6))]).unwrap();
    assert!(left(&o) == switch(&o), "only switching, whichever door he opens");
    o.log.commit("t", "surer", vec![Op::set(win, AtLeast(0.7))]).unwrap();
    assert!(left(&o).is_never());
}

/// How likely a sample's truth is, where that's a matter of draws.
fn chance_of(o: &Opened, name: &str) -> recollect_graph::chance::Chance {
    use recollect_graph::chance::{Reader, Share, Term};
    Reader::new().chance(&[Share { weight: 1.0, value: Term::Rand(uncertain(o, name)) }])
}

#[test]
fn the_examples_of_each_kind_of_uncertain_number_say_what_they_claim() {
    use recollect_graph::chance::{Law, Reader, Share, Term, phi};
    let near = |got: f64, want: f64, what: &str| assert!((got - want).abs() < 1e-9, "{what}: {got} isn't {want}");

    // A bus at random keeps you waiting twice as long as one on a timetable, every ten minutes either way.
    let o = open("bus");
    near(o.number("Average wait, timetable"), 5.0, "timetable");
    near(o.number("Average wait, random"), 10.0, "random");
    assert_eq!(o.get::<ValueCell>("Over a quarter of an hour, timetable").0.0.cases()[0].1, &Val::Bool(false), "never, and shown to be");
    let long = chance_of(&o, "Over a quarter of an hour, random");
    assert!(long.exact);
    near(long.chance, (-1.5f64).exp(), "a long wait");

    // A six-pack's wobbles partly cancel, and one bottle counted six times doesn't.
    let o = open("bottles");
    let mut reader = Reader::new();
    let mut law = |name: &str| reader.law(&uncertain(&o, name)).unwrap_or_else(|| panic!("{name} has a law"));
    let Law::Normal { mean, sd } = law("A six-pack") else { panic!("a six-pack is a normal") };
    near(mean, 3012.0, "a six-pack");
    near(sd, 1.5 * 6f64.sqrt(), "a six-pack's spread");
    assert_eq!(law("One bottle, six times"), Law::Normal { mean: 3012.0, sd: 9.0 });
    near(chance_of(&o, "Short of 500 ml").chance, phi(-2.0 / 1.5), "one bottle");
    near(chance_of(&o, "Short of 3 litres, six times over").chance, phi(-2.0 / 1.5), "one bottle, six times");
    let pack = chance_of(&o, "A six-pack short of 3 litres");
    assert!(pack.exact && pack.chance < 1.0 / 1500.0 && pack.chance > 1.0 / 2500.0, "{pack:?}");

    // Halving and doubling are as likely as each other, and the average is above the middle.
    let o = open("sales");
    near(o.number("Average sales"), 50000.0 * (0.7f64 * 0.7 / 2.0).exp(), "average");
    let (half, double) = (chance_of(&o, "Under half"), chance_of(&o, "Over double"));
    assert!(half.exact && double.exact);
    near(half.chance, phi(0.5f64.ln() / 0.7), "half");
    near(double.chance, half.chance, "double");
    let loss = chance_of(&o, "Makes a loss");
    assert!(loss.exact, "a lognormal moved along is still read exactly");
    near(loss.chance, phi((10000.0f64 / 0.3 / 50000.0).ln() / 0.7), "a loss");

    // Each leg is likely, and all three together much less so.
    let o = open("connection");
    near(truth(&o.get::<ValueCell>("Everything on time")).prob(), 0.85 * 0.9 * 0.95, "all on time");
    let odds = o.get::<ValueCell>("Chance of making it").0.0;
    let mut each: Vec<f64> = odds.cases().iter().map(|(_, v)| if let Val::Num(n) = v { *n } else { panic!("{v:?}") }).collect();
    each.sort_by(f64::total_cmp);
    assert_eq!(each.len(), 2, "one for each answer to the later train");
    near(each[0], 0.85 * 0.9 * 0.95, "the earlier train");
    near(each[1], 0.85 * 0.9, "the later train");

    // Adding the likeliest of each underestimates a total of jobs that lean towards overrunning.
    let o = open("painting");
    let total = Reader::new().summary(&[Share { weight: 1.0, value: Term::Rand(uncertain(&o, "Total")) }]);
    assert!(!total.exact && (total.mean - 8.5).abs() < 3.0 * total.error, "{total:?}");
    assert!(total.mean > o.number("Likeliest of each, added"));
    let day = chance_of(&o, "Done in a working day");
    assert!(!day.exact && day.chance < 0.5 && day.chance > 0.25, "{day:?}");
}

#[test]
fn the_drawbridge_is_quicker_on_average_and_worse_when_you_need_to_be_sure() {
    use recollect_graph::Op;
    use recollect_graph::chance::{Reader, Share, Term};
    use recollect_systems::formulas::{Chosen, known_draws, worlds_left};
    use recollect_systems::plan::KNOWING;
    let mut o = open("drawbridge");

    // Over the bridge, taken together, has two humps, and its average is between them.
    let bridge = o.get::<ValueCell>("Over the bridge").0.0;
    assert_eq!(bridge.len(), 2, "one for each way the bridge could be");
    let shares: Vec<Share> = bridge.cases().iter().map(|(when, v)| Share { weight: when.prob(), value: match v { Val::Rand(r) => Term::Rand(*r), other => panic!("{other:?}") } }).collect();
    let together = Reader::new().summary(&shares);
    let bars = &together.hist.bars;
    let peaks = (1..bars.len() - 1).filter(|&i| bars[i] > bars[i - 1] && bars[i] >= bars[i + 1] && bars[i] > 0.01).count();
    assert_eq!(peaks, 2, "two humps: {bars:?}");
    assert!((together.mean - (25.0 + 0.4 * 40.0 / 3.0)).abs() < 0.2, "{together:?}");
    let near = o.get::<ValueCell>("Within two minutes of 30").0.0;
    let near: f64 = near.cases().iter().map(|(when, v)| when.prob() * match v {
        Val::Bool(b) => f64::from(u8::from(*b)),
        Val::Rand(r) => Reader::new().chance(&[Share { weight: 1.0, value: Term::Rand(*r) }]).chance,
        other => panic!("{other:?}"),
    }).sum();
    assert!(near < 0.06, "you're hardly ever near the average: {near}");

    // For each way: the bridge is quicker on average, and less sure to be on time with 40 minutes.
    let read = |o: &Opened, name: &str, option: &str| -> f64 {
        let chosen = o.get::<Chosen>("Which way?").0.0;
        let world = chosen.cases().iter().find(|(_, v)| matches!(v, Val::Ref(e) if recollect_systems::label_of(o.g(), *e) == option)).map(|(when, _)| when.clone()).unwrap();
        let value = o.get::<ValueCell>(name).0.0;
        let found = value.cases().into_iter().find(|(when, _)| !(when & &world).is_never()).unwrap().1.clone();
        match found {
            Val::Num(n) => n,
            Val::Estimate(e) => e.value,
            other => panic!("{name} for {option} is {other:?}"),
        }
    };
    let (bridge_mean, ring_mean) = (read(&o, "Average journey", "Over the bridge"), read(&o, "Average journey", "Round the ring road"));
    assert!(bridge_mean < ring_mean && (ring_mean - 33.0).abs() < 1e-9, "{bridge_mean} {ring_mean}");
    let (bridge_odds, ring_odds) = (read(&o, "Chance of being on time", "Over the bridge"), read(&o, "Chance of being on time", "Round the ring road"));
    assert!(bridge_odds > 0.85 && bridge_odds < 0.93 && ring_odds > 0.98, "{bridge_odds} {ring_odds}");

    let left = |o: &Opened| worlds_left(o.g(), &Space::global().picks(&BTreeMap::new()), &Space::global().always(), &mut Reader::given(known_draws(o.g())));
    let takes = |o: &Opened, option: &str| o.get::<Chosen>("Which way?").0.0.holds(|v| matches!(v, Val::Ref(e) if recollect_systems::label_of(o.g(), *e) == option));
    assert!(left(&o) == takes(&o, "Round the ring road"), "only the ring road is sure enough");

    // In a hurry, the bridge is the better bet.
    let (have, on_time) = (o.id("Minutes you have"), o.id("On time"));
    o.log.commit("t", "in a hurry", vec![Op::set(have, ValueCell::num(33.0)), Op::set(on_time, recollect_systems::formulas::AtLeast(0.55))]).unwrap();
    assert!(left(&o) == takes(&o, "Over the bridge"));

    // Seeing the bridge before you choose: the bridge when it's down, and either when it's up as long as it's the ring road.
    o.log.commit("t", "back to 40", vec![Op::set(have, ValueCell::num(40.0)), Op::set(on_time, recollect_systems::formulas::AtLeast(0.95))]).unwrap();
    o.log.commit("t", "look out of the window", vec![Op::link(o.id("Which way?"), o.id("The bridge is up"), KNOWING)]).unwrap();
    let up = o.get::<ValueCell>("The bridge is up").0.0.holds(|v| *v == Val::Bool(true));
    assert!(left(&o) == (&takes(&o, "Round the ring road") | &(&takes(&o, "Over the bridge") & &!&up)), "the bridge when it's down, and the ring road either way");
}
