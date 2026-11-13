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
    let end = o.get::<Finish>("How do we publish it?").0.0;
    let at = |option: &str| *end.at(&BTreeMap::from([(o.id("How do we publish it?").to_string(), o.id(option).to_string())]));
    assert_eq!((at("As a printed booklet"), at("As a PDF"), at("As a website"), at("Something else")), (Some(5.0), Some(7.0), Some(4.0), None));
    assert!(matches!(plan::pressure(g, o.id("How do we publish it?")).unwrap(), Pressure::Free(_)));

    let o = open("critical-path");
    let slack = |n: &str| *o.get::<Slack>(n).0.0.at(&BTreeMap::new());
    assert_eq!(slack("Wire the rooms"), Some(2.0));
    assert_eq!(slack("Frame the walls"), Some(0.0));
    assert_eq!(*o.get::<Finish>("Paint").0.0.at(&BTreeMap::new()), Some(9.0));
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
