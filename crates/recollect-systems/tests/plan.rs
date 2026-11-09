//! Each scenario states what the planning system should do, going through the real write path.
//! Where the answer is a deliberate choice rather than the obvious one, the test says why.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use recollect_graph::worlds::{Bool, Cond, Space, Worldly};
use recollect_graph::{Cell, CellType, Cx, Dep, Entity, Error, Graph, Log, Name, On, Op, Registry, Stored, Target, Violation, cell};
use recollect_systems::expr::Val;
use recollect_systems::formulas::{Called, LABEL};
use recollect_systems::notes;
use recollect_systems::plan::{self, *};
use recollect_systems::view::ViewGraph;
use serde_json::json;

/// What a workspace with a plan has: notes, so ideas can relate, and the plan.
fn install(reg: &mut Registry) {
    notes::install(reg);
    plan::install(reg);
}

/// Write a value in a cell of the edge from one node to another.
fn on_edge<C: CellType>(from: Entity, to: Entity, value: C) -> Op {
    Op::Set { on: On::Edge { from, to }, cell: C::NAME.to_string(), to: Some(Stored::Value(serde_json::to_value(value).expect("cells serialise"))) }
}

/// Take away what a node says in a cell.
fn clear(id: Entity, cell: &str) -> Op {
    Op::Set { on: On::Node { id }, cell: cell.to_string(), to: None }
}

struct G {
    log: Log,
    ids: BTreeMap<String, Entity>,
    names: HashMap<String, String>,
    /// The nodes something has committed.
    made: BTreeSet<Entity>,
}

impl G {
    fn new() -> Self {
        Self::with(install)
    }

    fn with(install: impl FnOnce(&mut Registry)) -> Self {
        let mut reg = Registry::new();
        install(&mut reg);
        G { log: Log::new(Arc::new(reg)), ids: BTreeMap::new(), names: HashMap::new(), made: BTreeSet::new() }
    }

    /// The handle for this name. It's only a node once something commits it.
    fn e(&mut self, name: &str) -> Entity {
        if let Some(e) = self.ids.get(name) {
            return *e;
        }
        let e = Entity::new();
        self.ids.insert(name.to_string(), e);
        self.names.insert(e.to_string(), name.to_string());
        e
    }

    /// What makes this node, with its name, if nothing has yet.
    fn make(&mut self, name: &str) -> Vec<Op> {
        let e = self.e(name);
        if !self.made.insert(e) {
            return vec![];
        }
        vec![Op::Spawn { id: e }, Op::set(e, Name(name.to_string()))]
    }

    fn g(&self) -> &Graph {
        self.log.graph()
    }

    fn commit(&mut self, ops: Vec<Op>) -> u64 {
        self.log.commit("test", "setup", ops).expect("valid change")
    }

    fn node(&mut self, name: &str) -> &mut Self {
        let ops = self.make(name);
        if !ops.is_empty() {
            self.commit(ops);
        }
        self
    }

    fn add(&mut self, name: &str, component: &str) -> &mut Self {
        let mut ops = self.make(name);
        ops.push(Op::add(self.id(name), component));
        self.commit(ops);
        self
    }

    fn task(&mut self, name: &str) -> &mut Self {
        self.add(name, TASK)
    }

    fn decision(&mut self, name: &str) -> &mut Self {
        self.add(name, DECISION)
    }

    /// A decision with a gap for the options nobody has named yet.
    fn open_decision(&mut self, name: &str) -> &mut Self {
        let gap = format!("{name}.unknown");
        let mut ops = self.make(name);
        ops.extend(self.make(&gap));
        ops.extend(new_decision(self.id(name), self.id(&gap)));
        self.commit(ops);
        self
    }

    /// Say a node isn't fully specified yet, with a gap of this name.
    fn incomplete(&mut self, name: &str, gap: &str) -> &mut Self {
        let mut ops = self.make(gap);
        ops.extend(mark_incomplete(self.id(name), self.id(gap)));
        self.commit(ops);
        self
    }

    fn link(&mut self, from: &str, to: &str, component: &str) -> &mut Self {
        let mut ops = self.make(from);
        ops.extend(self.make(to));
        ops.push(Op::link(self.id(from), self.id(to), component));
        self.commit(ops);
        self
    }

    fn part_of(&mut self, from: &str, to: &str) -> &mut Self {
        self.link(from, to, PART_OF)
    }

    fn option(&mut self, from: &str, to: &str) -> &mut Self {
        self.link(from, to, ALTERNATIVE)
    }

    fn rule_out(&mut self, from: &str, to: &str) -> &mut Self {
        self.option(from, to);
        let (a, b) = (self.id(from), self.id(to));
        self.commit(vec![on_edge(a, b, RuledOut(json!({})))]);
        self
    }

    fn depends(&mut self, from: &str, to: &str) -> &mut Self {
        self.link(from, to, DEPENDS_ON)
    }

    fn waits_for_answer(&mut self, from: &str, to: &str) -> &mut Self {
        self.depends(from, to);
        let (a, b) = (self.id(from), self.id(to));
        self.commit(vec![on_edge(a, b, WaitFor::Decided)]);
        self
    }

    fn done(&mut self, name: &str) -> &mut Self {
        let e = self.e(name);
        self.commit(vec![Op::set(e, Done::yes())]);
        self
    }

    fn drop(&mut self, name: &str) -> &mut Self {
        let e = self.e(name);
        self.commit(vec![Op::set(e, Dropped::yes())]);
        self
    }

    fn choose(&mut self, decision: &str, option: &str) -> &mut Self {
        let (d, o) = (self.e(decision), self.e(option));
        self.commit(vec![choose(d, o)]);
        self
    }

    fn reopen(&mut self, decision: &str) -> &mut Self {
        let d = self.e(decision);
        self.commit(vec![reopen(d)]);
        self
    }

    fn id(&self, name: &str) -> Entity {
        *self.ids.get(name).unwrap_or_else(|| panic!("no node named {name}"))
    }

    fn name_of(&self, e: Entity) -> String {
        self.names.get(&e.to_string()).cloned().unwrap_or_else(|| e.to_string())
    }

    fn show(&self, b: &Bool) -> String {
        b.show_with(&|s| self.names.get(s).cloned().unwrap_or_else(|| s.to_string()))
    }

    fn need_of(&self, name: &str) -> Bool {
        self.g().get::<Need>(self.id(name)).unwrap().unwrap().0
    }

    fn need(&self, name: &str) -> String {
        self.show(&self.need_of(name))
    }

    /// The worlds in which something here is still owed.
    fn open(&self, name: &str) -> String {
        self.show(&self.g().get::<Done>(self.id(name)).unwrap().unwrap().open())
    }

    fn finished(&self, name: &str) -> String {
        self.show(&self.g().get::<Done>(self.id(name)).unwrap().unwrap().finished())
    }

    fn blocked(&self, name: &str) -> String {
        self.show(&self.g().get::<Blocked>(self.id(name)).unwrap().unwrap().0)
    }

    fn blockers(&self, name: &str) -> Vec<(String, String)> {
        self.g().get::<Blockers>(self.id(name)).unwrap().unwrap().0.iter().map(|(e, f)| (self.name_of(*e), self.show(f))).collect()
    }

    fn actionable(&self, name: &str) -> Option<String> {
        self.g().get::<Actionable>(self.id(name)).unwrap().map(|a| self.show(&a.0))
    }

    /// What's safe to start now, in every world.
    fn safe(&self) -> Vec<String> {
        self.g().entities().filter(|e| self.g().get::<Actionable>(*e).unwrap().is_some_and(|a| a.0.is_always())).map(|e| self.name_of(e)).collect()
    }

    fn pressure(&self, decision: &str) -> Pressure {
        plan::pressure(self.g(), self.id(decision)).unwrap()
    }

    fn notices(&self) -> Vec<(String, String)> {
        let on = |v: &Violation| match v.on {
            Target::Node(e) => self.name_of(e),
            _ => String::new(),
        };
        self.log.notices().iter().map(|v| (v.rule.clone(), on(v))).collect()
    }

    fn scope(&self, name: &str) -> Option<String> {
        self.g().get::<Scope>(self.id(name)).unwrap().map(|s| self.name_of(s.0))
    }

    fn entity_names(&self, nodes: impl Iterator<Item = Entity>) -> Vec<String> {
        nodes.map(|e| self.name_of(e)).collect()
    }
}

/// Three approaches with overlapping work.
fn calibration() -> G {
    let mut g = G::new();
    g.decision("calib");
    for o in ["cad", "board", "free"] {
        g.node(o).option(o, "calib");
    }
    for t in ["imu", "residual"] {
        g.task(t).part_of(t, "calib");
    }
    g.task("static").part_of("static", "cad").part_of("static", "free");
    g.task("assoc").part_of("assoc", "cad").part_of("assoc", "free").depends("assoc", "static");
    g.task("export").part_of("export", "cad");
    g.task("detector").part_of("detector", "board");
    g.task("search").part_of("search", "free").depends("search", "assoc");
    g
}

// ---------- needed ----------

#[test]
fn what_is_needed_is_exact() {
    let g = calibration();
    assert!(g.need_of("imu").is_always(), "every option needs it");
    assert_eq!(g.need("static"), "calib∈{cad,free}");
    assert_eq!(g.need("detector"), "calib=board");
    assert!((g.need_of("static").prob() - 2.0 / 3.0).abs() < 1e-9);
    assert!(g.need_of("calib").is_always());
}

#[test]
fn safe_work_and_pressure() {
    let g = calibration();
    assert_eq!(g.safe(), vec!["imu", "residual"]);
    assert_eq!(g.actionable("assoc").unwrap(), "never", "it waits for static in every world that needs it");
    assert_eq!(g.pressure("calib"), Pressure::Free(vec![g.id("imu"), g.id("residual")]));

    let mut g = calibration();
    g.done("imu").done("residual");
    assert!(matches!(g.pressure("calib"), Pressure::Blocking(_)), "only option-specific work is left");
}

#[test]
fn correlated_paths_are_exact_and_read_from_the_outside_in() {
    // Assuming paths are independent, which the current app does, gives 62.5% rather than 75%.
    let mut g = G::new();
    g.decision("root").node("a").option("a", "root").node("b").option("b", "root");
    g.decision("x").part_of("x", "a").node("p").option("p", "x").node("q").option("q", "x");
    g.task("y").part_of("y", "p").part_of("y", "b");
    assert!((g.need_of("y").prob() - 0.75).abs() < 1e-9);
    assert_eq!(g.need("y"), "(root=a ∧ x=p) ∨ root=b");
}

#[test]
fn an_outer_decision_comes_first_even_if_the_inner_one_is_asked_about_first() {
    let mut g = G::new();
    g.decision("root").node("a").option("a", "root").node("b").option("b", "root");
    g.decision("x").part_of("x", "a").node("p").option("p", "x").node("q").option("q", "x");
    g.task("y").part_of("y", "p").part_of("y", "b");
    g.g().get::<Done>(g.id("x")).unwrap();
    assert_eq!(g.need("y"), "(root=a ∧ x=p) ∨ root=b");
}

#[test]
fn ruling_out_narrows_the_worlds() {
    let mut g = calibration();
    g.rule_out("board", "calib");
    assert!(g.need_of("static").is_always(), "cad and free both need it, and they're all that's left");
    assert_eq!(g.need("detector"), "never", "pruned, with nothing stored on detector");
    assert!(!g.g().has_own(g.id("detector"), Dropped::NAME), "nobody said anything about it");
}

#[test]
fn weights_are_on_the_link_and_a_gap_has_none_unless_given_one() {
    let mut g = G::new();
    g.open_decision("d");
    g.option("a", "d").option("b", "d");
    let (a, b, d) = (g.id("a"), g.id("b"), g.id("d"));
    g.commit(vec![on_edge(a, d, Weight(Worldly::plain(Some(3.0)))), on_edge(b, d, Weight(Worldly::plain(Some(1.0))))]);
    assert!((g.need_of("a").prob() - 0.75).abs() < 1e-9);
    assert!((g.need_of("d.unknown").prob()).abs() < 1e-9);
}

// ---------- the bug that started this: reopening ----------

#[test]
fn deciding_and_reopening_never_touch_the_work() {
    let mut g = calibration();
    let saved = g.g().save();
    let before = g.need("detector");

    g.choose("calib", "cad");
    assert_eq!(g.need("detector"), "never");
    assert!(g.need_of("export").is_always());

    g.reopen("calib");
    assert_eq!(g.need("detector"), before, "everything comes back on its own");
    assert_eq!(g.g().save(), saved, "and no fact changed at any point");

    g.choose("calib", "board");
    assert!(g.need_of("detector").is_always());
    assert_eq!(g.need("export"), "never");
}

// ---------- completion ----------

#[test]
fn completion_knows_about_choices() {
    let mut g = calibration();
    g.done("imu").done("residual");
    assert_eq!(g.finished("calib"), "never", "no option is complete yet");

    // Finish one approach: done if you pick it, and not otherwise.
    g.done("static").done("assoc").done("export");
    assert_eq!(g.finished("cad"), "always");
    assert_eq!(g.finished("calib"), "calib=cad");

    // Finish the others too: done in every world, without having decided.
    g.done("detector").done("search");
    assert_eq!(g.finished("board"), "always");
    assert_eq!(g.finished("calib"), "always", "complete whichever way it goes");
    assert_eq!(g.g().get::<Chosen>(g.id("calib")).unwrap(), None);
}

#[test]
fn a_node_with_nothing_to_finish_is_not_in_the_completion_system() {
    let mut g = G::new();
    g.node("note").task("t").depends("t", "note");
    let c = g.g().get::<Done>(g.id("note")).unwrap().unwrap();
    assert_eq!(c.0.0, Space::global().pure(None));
    assert_eq!(g.blocked("t"), "never", "a note requires nothing");

    g.incomplete("note", "note.gap");
    assert_eq!(g.blocked("t"), "always", "until it says there's more to come");
    let (note, gap) = (g.id("note"), g.id("note.gap"));
    g.commit(close_gap(gap, note, PART_OF));
    assert_eq!(g.blocked("t"), "never");
}

#[test]
fn a_summary_with_nothing_completable_beneath_it_is_not_in_the_system_either() {
    let mut g = G::new();
    g.node("folder").node("note").part_of("note", "folder");
    assert_eq!(g.g().get::<Done>(g.id("folder")).unwrap().unwrap().0.0, Space::global().pure(None));
    g.task("t").part_of("t", "folder");
    assert_eq!(g.open("folder"), "always", "a task under it brings it in");
    g.done("t");
    assert_eq!(g.finished("folder"), "always");
}

#[test]
fn a_dependency_says_what_it_waits_for() {
    let mut g = G::new();
    g.decision("d");
    for o in ["a", "b"] {
        g.task(o).option(o, "d");
    }
    g.task("after_work").depends("after_work", "d");
    g.task("after_choice").waits_for_answer("after_choice", "d");
    assert_eq!(g.blocked("after_work"), "always");
    assert_eq!(g.blocked("after_choice"), "always");

    // Finish option a: the work could be complete, depending on the decision.
    g.done("a");
    assert_eq!(g.blocked("after_work"), "d=b", "exactly what would unblock it");
    assert_eq!(g.blocked("after_choice"), "always");

    // Decide b instead: the choice dependency clears, and the work one doesn't.
    g.choose("d", "b");
    assert_eq!(g.blocked("after_choice"), "never");
    assert_eq!(g.blocked("after_work"), "always", "b isn't done");
}

#[test]
fn a_decision_that_is_only_a_decision() {
    // No work under the options at all: "which colour?". What keeps it open is the gap.
    let mut g = G::new();
    g.open_decision("colour");
    for o in ["red", "blue"] {
        g.node(o).option(o, "colour");
    }
    g.task("paint").depends("paint", "colour");
    g.task("mix").waits_for_answer("mix", "colour");
    assert_eq!(g.open("colour"), "colour=colour.unknown", "nothing's owed unless it's something nobody has named");
    assert_eq!(g.blocked("paint"), "colour=colour.unknown");
    assert_eq!(g.blocked("mix"), "always");

    // Closing the gap says these are all the options. Nothing's owed, and only a dependent that
    // waits for the answer still waits.
    let (colour, gap) = (g.id("colour"), g.id("colour.unknown"));
    g.commit(close_gap(gap, colour, ALTERNATIVE));
    assert_eq!(g.open("colour"), "never");
    assert_eq!(g.blocked("paint"), "never");
    assert_eq!(g.blocked("mix"), "always");

    g.choose("colour", "red");
    assert_eq!(g.open("colour"), "never");
    assert_eq!(g.blocked("mix"), "never");
}

#[test]
fn what_a_node_says_for_itself_replaces_what_is_beneath_it() {
    let mut g = G::new();
    g.add("goal", GOAL);
    g.task("t").part_of("t", "goal").task("after").depends("after", "goal");
    assert_eq!(g.open("goal"), "always", "it's worked out from t");
    assert_eq!(g.blocked("after"), "always");

    g.done("goal");
    assert_eq!(g.finished("goal"), "always", "until it says otherwise");
    assert_eq!(g.blocked("after"), "never");
    assert_eq!(g.notices(), vec![("stated_done_with_open_work".to_string(), "goal".to_string())], "and the disagreement is visible");

    // Saying what's being left out settles it, and it's a fact with an author too.
    g.drop("t");
    assert_eq!(g.notices(), vec![("dropped_part_of_live_work".to_string(), "t".to_string())]);

    // And without the statement, it's worked out again.
    let (t, goal) = (g.id("t"), g.id("goal"));
    g.commit(vec![clear(t, Dropped::NAME), clear(goal, Done::NAME)]);
    assert_eq!(g.open("goal"), "always");
}

/// A statement in the middle settles things for what's above it, so that's the one that disagrees.
#[test]
fn the_notice_goes_on_the_statement_that_disagrees() {
    let mut g = G::new();
    g.node("goal").node("phase").task("t").part_of("phase", "goal").part_of("t", "phase");
    g.done("phase").done("goal");
    assert_eq!(g.notices().iter().map(|n| n.1.as_str()).collect::<Vec<_>>(), vec!["phase"]);
    g.done("t");
    assert_eq!(g.notices(), vec![], "they agree now");
}

#[test]
fn a_done_leaf_is_not_a_contradiction() {
    let mut g = G::new();
    g.task("t").done("t");
    assert_eq!(g.notices(), vec![]);
    assert_eq!(g.finished("t"), "always");
}

#[test]
fn a_stated_done_decision_is_noted() {
    let mut g = calibration();
    g.done("calib");
    assert_eq!(g.finished("calib"), "always");
    assert_eq!(g.notices(), vec![("stated_done_with_open_work".to_string(), "calib".to_string())]);
}

// ---------- notices about how a plan is coming along ----------

#[test]
fn a_goal_with_nothing_under_it_is_worth_a_look() {
    let mut g = G::new();
    g.add("goal", GOAL);
    assert_eq!(g.notices(), vec![("undeveloped_goal".to_string(), "goal".to_string())]);
    g.task("t").part_of("t", "goal");
    assert_eq!(g.notices(), vec![]);

    let mut g = G::new();
    g.add("goal", GOAL).link("helper", "goal", "serves");
    assert_eq!(g.notices(), vec![], "something serving it counts too");
}

#[test]
fn a_decision_down_to_one_option_is_worth_a_look() {
    let mut g = G::new();
    g.decision("d").node("a").option("a", "d").node("b").option("b", "d");
    assert_eq!(g.notices(), vec![]);
    g.rule_out("b", "d");
    assert_eq!(g.notices(), vec![("one_option_left".to_string(), "d".to_string())]);
    g.choose("d", "a");
    assert_eq!(g.notices(), vec![], "not once it's decided");
}

#[test]
fn a_gap_isnt_an_option_you_could_be_told_to_take() {
    let mut g = G::new();
    g.open_decision("d");
    assert_eq!(g.notices(), vec![], "only a gap so far, and that can't be chosen");
    g.node("a").option("a", "d");
    assert_eq!(g.notices(), vec![], "one named option, and there may be more to come");
    let (d, gap) = (g.id("d"), g.id("d.unknown"));
    g.commit(close_gap(gap, d, ALTERNATIVE));
    assert_eq!(g.notices(), vec![("one_option_left".to_string(), "d".to_string())], "these are all there are, so it isn't a choice");
}

#[test]
fn a_risk_threatening_nothing_is_worth_a_look() {
    let mut g = G::new();
    g.add("risk", RISK).task("t");
    assert_eq!(g.notices(), vec![("risk_without_target".to_string(), "risk".to_string())]);
    g.link("risk", "t", "contradicts");
    assert_eq!(g.notices(), vec![]);
}

#[test]
fn a_big_task_with_no_breakdown_is_worth_a_look() {
    let mut g = G::new();
    g.task("t");
    let t = g.id("t");
    g.commit(vec![Op::set(t, Estimate::days(6.0))]);
    assert_eq!(g.notices(), vec![("oversized_task".to_string(), "t".to_string())]);
    g.task("part").part_of("part", "t");
    assert_eq!(g.notices(), vec![], "it has one now");
}

#[test]
fn an_estimate_that_varies_is_too_big_if_it_could_be_and_reads_as_a_range() {
    let mut g = G::new();
    g.decision("d").option("a", "d").option("b", "d").task("t");
    let (d, a, t) = (g.id("d"), g.id("a"), g.id("t"));
    g.commit(vec![
        Op::set_formula(t, Estimate::NAME, "if(chose(d, a), 9, 1)"),
        Op::link(t, d, LABEL),
        on_edge(t, d, Called("d".to_string())),
        Op::link(t, a, LABEL),
        on_edge(t, a, Called("a".to_string())),
    ]);
    assert_eq!(g.notices(), vec![("oversized_task".to_string(), "t".to_string())], "it's too big in one of the worlds");
    let read = |picks: &BTreeMap<String, String>| plan::work(g.g(), picks).unwrap().into_iter().find(|w| w.id == t).unwrap().estimate;
    assert_eq!(read(&BTreeMap::new()).as_deref(), Some("1 to 9"));
    assert_eq!(read(&world(&g, &[("d", "b")])).as_deref(), Some("1"));
}

// ---------- what a viewer asks ----------

fn world(g: &G, picks: &[(&str, &str)]) -> BTreeMap<String, String> {
    picks.iter().map(|(d, o)| (g.id(d).to_string(), g.id(o).to_string())).collect()
}

#[test]
fn work_says_where_each_task_stands_among_the_worlds_in_play() {
    let mut g = calibration();
    g.done("imu");
    let standing = |g: &G, picks: &[(&str, &str)]| -> BTreeMap<String, Standing> { plan::work(g.g(), &world(g, picks)).unwrap().into_iter().map(|w| (w.name, w.standing)).collect() };
    let open = standing(&g, &[]);
    assert_eq!(open["imu"], Standing::Finished);
    assert_eq!(open["residual"], Standing::Now);
    assert_eq!(open["static"], Standing::Depends);
    assert_eq!(open["assoc"], Standing::Waiting);

    let cad = standing(&g, &[("calib", "cad")]);
    assert_eq!(cad["static"], Standing::Now, "narrowing to a world, it isn't contingent any more");
    assert_eq!(cad["detector"], Standing::NotNeeded);
}

#[test]
fn a_matrix_says_what_each_option_needs() {
    let g = calibration();
    let m = plan::matrices(g.g(), &BTreeMap::new()).unwrap().remove(0);
    assert_eq!(m.name, "calib");
    assert_eq!(m.options.iter().map(|o| (o.name.as_str(), (o.weight * 3.0).round() as i32)).collect::<Vec<_>>(), vec![("cad", 1), ("board", 1), ("free", 1)]);
    let row = |n: &str| m.rows.iter().find(|r| r.name == n).unwrap();
    let per = |n: &str, o: &str| row(n).per_option[&g.id(o)];
    assert_eq!((per("static", "cad"), per("static", "board"), per("static", "free")), (1.0, 0.0, 1.0));
    assert_eq!(per("imu", "board"), 1.0);
    assert_eq!(row("static").need.when, "calib∈{cad,free}");
    assert_eq!(m.rows[0].need.prob, 1.0, "what's needed most comes first");
    assert!(matches!(m.pressure, Pressure::Free(_)));

    let narrowed = plan::matrices(g.g(), &world(&g, &[("calib", "board")])).unwrap().remove(0);
    let row = |n: &str| narrowed.rows.iter().find(|r| r.name == n).unwrap();
    assert!(row("static").need.never && row("detector").need.always, "a world narrows it");
}

// ---------- gaps ----------

#[test]
fn a_gap_keeps_a_decision_open_until_someone_closes_it() {
    let mut g = estimated();
    assert_eq!(g.need("static"), "calib∈{cad,free}", "safe only among the named options");
    assert!(!g.need_of("static").is_always() && g.need_of("static").holds_among_known());
    assert!(g.need_of("imu").is_always(), "needed even if the answer is something else entirely");
    assert!((g.need_of("static").prob() - 1.0).abs() < 1e-9, "a gap has no likelihood until given one");
    assert_eq!(g.actionable("calib.unknown"), None, "and it isn't work");
    // Named-option safety counts as safe, or a default gap would hide all shared work.
    assert_eq!(g.pressure("calib"), Pressure::Free(vec![g.id("imu"), g.id("static")]));

    for t in ["imu", "static", "export", "search"] {
        g.done(t);
    }
    assert_eq!(g.finished("calib"), "calib∈{cad,free}", "all known work finished, and it might not be all");

    let (calib, gap) = (g.id("calib"), g.id("calib.unknown"));
    let seq = g.log.commit("steven", "these are the only two approaches", close_gap(gap, calib, ALTERNATIVE)).unwrap();
    assert_eq!(g.finished("calib"), "always");
    let ev = &g.log.events()[seq as usize];
    assert_eq!((ev.author.as_str(), ev.why.as_str()), ("steven", "these are the only two approaches"), "closing it is an audited claim");

    g.log.undo(seq, "steven", "actually, maybe not").unwrap();
    assert_eq!(g.finished("calib"), "calib∈{cad,free}", "and reversible");
}

#[test]
fn an_option_with_no_work_next_to_a_finished_one() {
    let mut g = G::new();
    g.decision("d").node("a").option("a", "d").task("b").option("b", "d").done("b");
    assert_eq!(g.open("d"), "never");
    // An option that isn't thought through yet says so.
    g.incomplete("a", "a.gap");
    assert_eq!(g.open("d"), "d=a");
    g.choose("d", "a");
    assert_eq!(g.open("d"), "always", "choosing it doesn't specify it");
    let (a, gap) = (g.id("a"), g.id("a.gap"));
    g.commit(close_gap(gap, a, PART_OF));
    assert_eq!(g.open("d"), "never");
}

// ---------- nesting, sharing and dropping ----------

#[test]
fn nested_decisions_only_complete_when_their_chosen_path_does() {
    let mut g = G::new();
    g.decision("outer").node("p").option("p", "outer").node("q").option("q", "outer");
    g.decision("inner").part_of("inner", "p");
    g.task("x").option("x", "inner").task("y").option("y", "inner");
    g.task("z").part_of("z", "q");
    assert_eq!(g.need("x"), "outer=p ∧ inner=x");
    g.done("x");
    assert_eq!(g.finished("outer"), "outer=p ∧ inner=x");
    g.choose("inner", "x");
    assert_eq!(g.finished("outer"), "outer=p", "the inner decision is settled, the outer isn't");
    g.choose("outer", "q");
    assert_eq!(g.finished("outer"), "never", "the chosen path has undone work");
    assert_eq!(g.need("x"), "never", "and the other path's work is no longer needed");
}

#[test]
fn shared_work_survives_any_path_that_still_wants_it() {
    let mut g = calibration();
    g.rule_out("free", "calib");
    assert_eq!(g.need("static"), "calib=cad", "free is gone, cad still needs it");
    g.rule_out("cad", "calib");
    assert_eq!(g.need("static"), "never");
    // Only board left: it's forced in every remaining world, though nobody has chosen it.
    assert!(g.need_of("detector").is_always());
    assert_eq!(g.g().get::<Chosen>(g.id("calib")).unwrap(), None);
    // And it all reverses.
    let (cad, calib) = (g.id("cad"), g.id("calib"));
    g.commit(vec![Op::Set { on: On::Edge { from: cad, to: calib }, cell: RuledOut::NAME.to_string(), to: None }]);
    assert_eq!(g.need("static"), "calib=cad");
}

#[test]
fn no_options_left_is_visible() {
    let mut g = G::new();
    g.decision("d").node("a").option("a", "d").drop("a");
    assert_eq!(g.notices(), vec![("decision_without_options".to_string(), "d".to_string())]);
    assert_eq!(g.open("d"), "always", "it can't complete with nothing to choose");
}

#[test]
fn dropping_is_explicit_and_reversible() {
    let mut g = G::new();
    g.node("phase").task("a").task("b").part_of("a", "phase").part_of("b", "phase").done("a");
    assert_eq!(g.open("phase"), "always");
    g.drop("b");
    assert_eq!(g.finished("phase"), "always", "it no longer requires b");
    assert_eq!(g.notices(), vec![("dropped_part_of_live_work".to_string(), "b".to_string())], "so that isn't silent");
    assert_eq!(g.g().get::<Done>(g.id("b")).unwrap().unwrap().0.0, Space::global().pure(None), "dropped is neither done nor owed");
    let b = g.id("b");
    g.commit(vec![clear(b, Dropped::NAME)]);
    assert_eq!(g.open("phase"), "always");
    assert_eq!(g.notices(), vec![]);
}

#[test]
fn dropping_a_branch_prunes_below_it_except_shared_work() {
    let mut g = G::new();
    g.node("root").node("a").part_of("a", "root").node("b").part_of("b", "root");
    g.task("only_a").part_of("only_a", "a").task("both").part_of("both", "a").part_of("both", "b");
    g.drop("a");
    assert_eq!(g.need("only_a"), "never");
    assert!(g.need_of("both").is_always());
}

// ---------- blocking ----------

#[test]
fn a_dependency_on_work_that_is_not_needed_does_not_block() {
    let mut g = calibration();
    g.depends("export", "detector");
    // export is needed if cad, and detector only if board. They can't both be needed.
    assert_eq!(g.blocked("export"), "calib=board");
    assert_eq!(g.actionable("export").unwrap(), "calib=cad", "where it's needed, it isn't blocked");
}

#[test]
fn dependencies_are_inherited_from_ancestors() {
    let mut g = G::new();
    g.task("prereq").node("phase").task("step").part_of("step", "phase").depends("phase", "prereq");
    assert_eq!(g.blocked("step"), "always");
    assert_eq!(g.blockers("step"), vec![("prereq".to_string(), "always".to_string())], "and it says so");
    g.done("prereq");
    assert_eq!(g.blocked("step"), "never");
}

#[test]
fn blockers_are_named_with_their_conditions() {
    let mut g = calibration();
    g.depends("export", "detector");
    assert_eq!(g.blockers("search"), vec![("assoc".to_string(), "calib∈{cad,free}".to_string())]);
    assert_eq!(g.blockers("export"), vec![("detector".to_string(), "calib=board".to_string())]);
}

/// The bug `recollect-kernel` has: there, `shared` would wait for `prereq` even if you take b.
#[test]
fn shared_work_only_inherits_from_parents_in_play() {
    let mut g = G::new();
    g.decision("d").node("a").option("a", "d").node("b").option("b", "d");
    g.task("prereq").depends("a", "prereq");
    g.task("shared").part_of("shared", "a").part_of("shared", "b");
    assert_eq!(g.blocked("shared"), "d=a", "taking b means not waiting");
    assert_eq!(g.actionable("shared").unwrap(), "d=b");
    assert_eq!(g.blockers("shared"), vec![("prereq".to_string(), "d=a".to_string())]);
}

/// Another kernel bug: there, whichever route it reaches last replaces the other, so `after` would
/// wait only where the plain dependency does.
#[test]
fn the_same_blocker_by_two_routes_holds_in_either() {
    let mut g = G::new();
    g.decision("d").task("a").option("a", "d").task("b").option("b", "d").done("b");
    g.node("phase").depends("phase", "d");
    g.task("after").part_of("after", "phase").waits_for_answer("after", "d");
    assert_eq!(g.blockers("after"), vec![("d".to_string(), "always".to_string())], "it wants the answer, and that's still open");

    let mut g = G::new();
    g.decision("d").node("a").option("a", "d").node("b").option("b", "d");
    g.task("prereq");
    g.task("shared").part_of("shared", "a").part_of("shared", "b");
    g.depends("a", "prereq").depends("b", "prereq");
    assert_eq!(g.blockers("shared"), vec![("prereq".to_string(), "always".to_string())]);
}

#[test]
fn a_dropped_or_ruled_out_parent_passes_nothing_down() {
    let mut g = G::new();
    g.task("prereq").decision("d").node("a").option("a", "d").node("b").option("b", "d").depends("a", "prereq");
    g.task("y").part_of("y", "a");
    assert_eq!(g.blocked("y"), "d=a");
    g.rule_out("a", "d");
    assert_eq!(g.blocked("y"), "never", "a is never taken");

    g.node("phase").node("other").depends("phase", "prereq");
    g.task("z").part_of("z", "phase").part_of("z", "other");
    assert_eq!(g.blocked("z"), "always");
    g.drop("phase");
    assert_eq!(g.blocked("z"), "never", "and z is still needed, through other");
    assert!(g.need_of("z").is_always());
}

// ---------- audit, what-if and pure functions ----------

#[test]
fn a_past_state_derives_what_it_derived_then() {
    let mut g = calibration();
    let (calib, cad) = (g.id("calib"), g.id("cad"));
    let seq = g.log.commit("steven", "CAD model is already accurate", vec![choose(calib, cad)]).unwrap();
    let then = g.log.state_at(seq - 1);
    let need = then.get::<Need>(g.id("detector")).unwrap().unwrap().0;
    assert_eq!(g.show(&need), "calib=board");
    assert_eq!(g.need("detector"), "never");
}

#[test]
fn what_if_and_commit_share_one_code_path() {
    let mut g = calibration();
    let saved = g.g().save();
    let events = g.log.events().len();
    let (calib, cad, board) = (g.id("calib"), g.id("cad"), g.id("board"));

    // The option matrix is just each option's hypothetical.
    let if_cad = g.log.what_if(&[choose(calib, cad)]).unwrap();
    let if_board = g.log.what_if(&[choose(calib, board)]).unwrap();
    let need = |h: &Graph, n: &str| h.get::<Need>(g.id(n)).unwrap().unwrap().0;
    assert!(need(&if_cad, "export").is_always() && need(&if_cad, "detector").is_never());
    assert!(need(&if_board, "detector").is_always() && need(&if_board, "export").is_never());
    assert_eq!(g.g().save(), saved, "previewing writes nothing");
    assert_eq!(g.log.events().len(), events, "and logs nothing");

    // What a confirm step would show is a comparison of two derivations.
    let safe = |h: &Graph| -> BTreeSet<String> { h.entities().filter(|e| h.get::<Actionable>(*e).unwrap().is_some_and(|a| a.0.is_always())).map(|e| g.name_of(e)).collect() };
    let became_safe: Vec<String> = safe(&if_cad).difference(&safe(g.g())).cloned().collect();
    assert_eq!(became_safe, vec!["export", "static"]);

    // A preview agrees exactly with really doing it.
    let (imu, ids): (Entity, Vec<Entity>) = (g.id("imu"), g.g().entities().collect());
    let preview: Vec<_> = ids.iter().map(|e| if_cad.get::<Actionable>(*e).unwrap()).collect();
    g.choose("calib", "cad");
    assert_eq!(ids.iter().map(|e| g.g().get::<Actionable>(*e).unwrap()).collect::<Vec<_>>(), preview);

    // An impossible hypothetical is refused the same way a real change would be.
    let Err(refused) = g.log.what_if(&[choose(calib, imu)]) else { panic!("it isn't one of its options") };
    assert_eq!(refused[0].rule, "chosen_must_be_a_live_option");
}

#[test]
fn the_reads_say_what_each_value_came_from() {
    let g = estimated();
    g.g().get::<Done>(g.id("calib")).unwrap();
    let reads = g.g().reads_of(g.id("calib"), Done::NAME);
    for child in ["imu", "cad", "free", "calib.unknown"] {
        assert!(reads.contains(&Dep::Cell((g.id(child), Done::NAME.to_string()))), "{child}: {reads:?}");
    }
}

#[test]
fn an_edit_reruns_only_what_it_reaches() {
    let mut g = calibration();
    for e in g.g().entities().collect::<Vec<_>>() {
        g.g().get::<Actionable>(e).unwrap();
    }
    g.g().take_ran();
    g.done("detector");
    let actionable: Vec<Option<Actionable>> = g.g().entities().map(|e| g.g().get::<Actionable>(e).unwrap()).collect();
    assert_eq!(actionable.len(), g.g().entities().count());
    let ran: Vec<String> = g.g().take_ran().into_iter().map(|(e, c)| format!("{c}({})", g.name_of(e))).collect();
    assert!(!ran.iter().any(|r| r.starts_with("need") || r.starts_with("choice") || r.contains("export") || r.contains("static")), "{ran:?}");
    assert!(ran.contains(&"actionable(detector)".to_string()), "{ran:?}");
}

#[test]
fn explaining_gives_one_level_of_exact_inputs() {
    let mut g = calibration();
    g.depends("export", "detector");
    let text = plan::explain(g.g(), g.id("export"), &|s| g.names.get(s).cloned().unwrap_or_else(|| s.to_string())).unwrap();
    assert_eq!(text, "export\n  need: calib=cad\n  open: always\n  blocked: calib=board\n  waits for detector: calib=board\n");
}

#[test]
fn pressure_only_applies_to_an_open_decision() {
    let mut g = calibration();
    g.choose("calib", "cad");
    assert_eq!(g.pressure("calib"), Pressure::Decided);
    assert_eq!(g.pressure("imu"), Pressure::Decided, "it isn't a decision at all");
    let mut g = G::new();
    g.decision("d").node("a").option("a", "d");
    assert_eq!(g.pressure("d"), Pressure::Idle);
}

// ---------- hand edits ----------

#[test]
fn a_loop_from_a_hand_edit_is_reported_and_the_rest_still_works() {
    let mut reg = Registry::new();
    install(&mut reg);
    let reg = Arc::new(reg);
    let mut plain = Graph::with_registry(reg.clone());
    let (a, b, c) = (plain.spawn(), plain.spawn(), plain.spawn());
    plain.add(c, TASK);
    plain.link(a, b, PART_OF);
    plain.link(b, a, PART_OF);
    let mut log = Log::new(reg);
    let broken = log.reconcile(&plain.save(), "hand").unwrap();
    assert!(broken.iter().any(|v| v.rule == "no_deadlock"), "{broken:?}");
    assert!(matches!(log.graph().get::<Need>(a), Err(Error::Cycle(_))), "reported in the cell");
    assert!(log.graph().get::<Need>(c).unwrap().unwrap().0.is_always(), "and the rest of the graph is fine");
    let v = ViewGraph::of(log.graph()).collapse(log.graph(), &BTreeSet::new());
    assert_eq!(v.nodes.len(), 3, "a view can still be drawn");
}

/// A commit can't do this, because `chosen_must_be_a_live_option` refuses it.
#[test]
fn a_hand_edit_choosing_a_ruled_out_option_lands_on_nothing() {
    let mut g = G::new();
    g.decision("d").node("a").option("a", "d").node("b").option("b", "d");
    g.task("work").part_of("work", "a").done("work").choose("d", "a");
    assert_eq!(g.finished("d"), "always");

    let (a, d) = (g.id("a"), g.id("d"));
    let mut files = g.g().save();
    let link = files.nodes.iter_mut().find(|n| n.id == a).unwrap().links.iter_mut().find(|l| l.to == d).unwrap();
    link.cells.insert(RuledOut::NAME.to_string(), json!({}));
    let broken = g.log.reconcile(&files, "hand").unwrap();
    assert_eq!(broken[0].rule, "chosen_must_be_a_live_option");

    assert_eq!(g.need("a"), "never", "ruled out still means out");
    assert_eq!(g.need("work"), "never");
    assert_eq!(g.open("d"), "always", "and its finished work doesn't complete the decision");
}

/// What `done` says is written by hand here, so it's the value that's malformed now, where it was
/// the task component before.
#[test]
fn a_malformed_value_is_an_error_in_what_reads_it_not_everywhere() {
    let mut g = estimated();
    let imu = g.id("imu");
    let mut files = g.g().save();
    files.nodes.iter_mut().find(|n| n.id == imu).unwrap().cells.insert(Done::NAME.to_string(), json!("soon"));
    let broken = g.log.reconcile(&files, "hand").unwrap();
    assert_eq!(broken[0].rule, "malformed_cell");
    assert!(matches!(g.g().get::<Actionable>(imu), Err(Error::Malformed { .. })));
    assert!(g.need_of("static").holds_among_known(), "need doesn't read it");
    assert_eq!(g.actionable("export").unwrap(), "calib=cad", "and other work carries on");
}

// ---------- plugins: new uses without touching the planning system ----------

/// A use the system knows nothing about: which work is exposed to a risk that's still open.
const THREATENS: &str = "threatens";
const EXPOSURE: &str = "exposure";

#[cell]
struct Exposed(Bool);

fn exposed(cx: &Cx) -> Cell<Exposed> {
    let mut any_open = false;
    for risk in cx.sources(THREATENS) {
        any_open |= !cx.get::<Done>(risk)?.is_some_and(|d| d.finished().is_always()) && !cx.get::<Dropped>(risk)?.is_some_and(|d| d.0.0.holds(|v| *v == Some(true)).is_always());
    }
    Ok(Some(Exposed(if any_open { cx.own_value::<Need>()?.0 } else { Space::global().never() })))
}

#[test]
fn a_new_use_is_a_new_component_with_a_cell_of_its_own() {
    let mut g = G::with(|reg| {
        install(reg);
        reg.component(EXPOSURE, "Threatened by a risk.").requires(PART);
        reg.cell::<Exposed>(EXPOSURE, "The worlds where an open risk threatens this.").builtin(exposed);
        reg.edge(THREATENS, "Threatens that.").target_has(EXPOSURE);
    });
    for o in ["cad", "free"] {
        g.node(o);
    }
    g.decision("calib").option("cad", "calib").option("free", "calib");
    g.task("search").part_of("search", "free");
    g.add("diverges", RISK).link("diverges", "search", THREATENS);
    let search = g.id("search");
    let exposed = g.g().get::<Exposed>(search).unwrap().unwrap().0;
    assert_eq!(g.show(&exposed), "calib=free", "only exposed if you take the option that needs it");
    assert!(g.g().reads_of(search, Exposed::NAME).contains(&Dep::Cell((search, Need::NAME.to_string()))), "nothing declared that it reads need: the engine saw it");
    assert_eq!(g.need("search"), "calib=free", "and the new components change nothing the system computes");
}

/// The workspace's own cells, and edges the plan doesn't read, are kept as given and change nothing.
#[test]
fn what_this_system_does_not_read_is_kept_and_ignored() {
    let mut g = estimated();
    let (imu, search) = (g.id("imu"), g.id("search"));
    let before = (g.need("static"), g.open("calib"), g.blocked("search"));
    g.commit(vec![Op::set_value(imu, "review", json!({ "by": "ana", "pages": 12 })), Op::link(imu, search, "relates"), Op::Set { on: On::Edge { from: imu, to: search }, cell: "strength".to_string(), to: Some(Stored::Value(json!(0.5))) }]);
    assert_eq!(g.g().get_value(imu, "review").unwrap().unwrap()["by"], "ana", "stored as given");
    assert_eq!(g.g().get_value(Entity::edge(imu, search), "strength").unwrap(), Some(json!(0.5)));
    assert_eq!((g.need("static"), g.open("calib"), g.blocked("search")), before);
}

/// Work is what's owed, so a plugin's component is owed when it requires work, and only then.
#[test]
fn a_plugin_can_add_an_obligation() {
    let doc = |owed: bool| {
        let mut g = G::with(|reg| {
            install(reg);
            let review = reg.component("review", "Something to read over.");
            if owed {
                review.requires(WORK);
            }
        });
        g.add("doc", "review").node("note");
        let get = |n: &str| g.g().get::<Done>(g.id(n)).unwrap().map(|d| g.show(&d.open()));
        (get("doc"), get("note"))
    };
    assert_eq!(doc(false), (None, None), "not declared as work, it's not in the plan at all");
    assert_eq!(doc(true), (Some("always".to_string()), None), "declared as work, it has to be finished");
}

/// Remaining effort, per world, with shared work counted once.
#[cell]
struct Remaining(Cond<f64>);

const EFFORT: &str = "effort";

fn remaining(cx: &Cx) -> Cell<Remaining> {
    let space = Space::global();
    let mut total = space.pure(0.0);
    for t in cx.scan(TASK) {
        let open = &cx.value::<Need>(t)?.0 & &cx.value::<Done>(t)?.open();
        let days = cx.get::<Estimate>(t)?.map_or_else(|| space.pure(Some(0.0)), |e| e.0.0).map(|d| d.unwrap_or(0.0));
        let term = Cond::when(&open, &days, &space.pure(0.0));
        total = total.zip(&term, |a, b| a + b);
    }
    Ok(Some(Remaining(total)))
}

/// Two approaches with shared work, and a gap, with estimates.
fn estimated_with(install: impl FnOnce(&mut Registry)) -> G {
    let mut g = G::with(install);
    g.open_decision("calib");
    for o in ["cad", "free"] {
        g.node(o).option(o, "calib");
    }
    for (t, estimate, parents) in [("imu", 1.0, vec!["calib"]), ("static", 2.0, vec!["cad", "free"]), ("export", 3.0, vec!["cad"]), ("search", 5.0, vec!["free"])] {
        g.task(t);
        let e = g.id(t);
        g.commit(vec![Op::set(e, Estimate::days(estimate))]);
        for p in parents {
            g.part_of(t, p);
        }
    }
    g
}

fn estimated() -> G {
    estimated_with(install)
}

#[test]
fn numbers_differ_exactly_between_worlds() {
    let mut g = estimated_with(|reg| {
        install(reg);
        reg.component(EFFORT, "What's left to do beneath it.");
        reg.cell::<Remaining>(EFFORT, "Days of work left, in each world.").builtin(remaining);
    });
    g.add("calib", EFFORT);
    let calib = g.id("calib");
    let remaining = |g: &G| g.g().get::<Remaining>(calib).unwrap().unwrap().0;
    let c = remaining(&g);
    let world = |o: &str| BTreeMap::from([(calib.to_string(), g.ids[o].to_string())]);
    assert_eq!((*c.at(&world("cad")), *c.at(&world("free")), *c.at(&world("calib.unknown"))), (6.0, 8.0, 1.0));
    assert!((c.expected() - 7.0).abs() < 1e-9, "probability-weighted, because that's what weights are for");

    g.choose("calib", "free");
    assert_eq!(remaining(&g), Space::global().pure(8.0), "once decided, one value");
}

/// Standing in for a TypeScript function: only strings and JSON cross this line. A cell with no
/// Rust type, worked out by a formula, reads the plan's cells the same way.
#[test]
fn a_cell_with_no_rust_type_can_read_and_combine_these() {
    let mut g = estimated();
    let export = g.id("export");
    g.commit(vec![Op::set_value(export, "pages", json!(12)), Op::set_formula(export, "review_pages", "if(self.need, self.pages, 0)")]);
    let value = g.g().get_value(export, "review_pages").unwrap().unwrap();
    let c: Worldly<Val> = serde_json::from_value(value).unwrap();
    let at = |o: &str| c.0.at(&BTreeMap::from([(g.id("calib").to_string(), g.ids[o].to_string())])).clone();
    assert_eq!((at("cad"), at("free")), (Val::Num(12.0), Val::Num(0.0)));
    assert!(g.g().reads_of(export, "review_pages").contains(&Dep::Cell((export, Need::NAME.to_string()))), "reads are tracked the same way");
}

// ---------- scope ----------

#[test]
fn scope_follows_the_structure() {
    let mut g = estimated();
    assert_eq!(g.scope("calib"), None);
    assert_eq!(g.scope("export"), Some("cad".to_string()), "only one parent: it lives inside it");
    assert_eq!(g.scope("cad"), Some("calib".to_string()));
    assert_eq!(g.scope("static"), Some("calib".to_string()), "two options share it: it lives in what they share");
    assert_eq!(g.g().get::<ScopeDepth>(g.id("export")), Ok(Some(ScopeDepth(2))));

    // Add a parent elsewhere and it moves out, with nothing to maintain.
    g.node("elsewhere").part_of("export", "elsewhere");
    assert_eq!(g.scope("export"), None, "two unrelated roots share nothing");
}

#[test]
fn deciding_moves_nothing() {
    let mut g = estimated();
    let all = |g: &G| g.g().entities().map(|e| g.g().get::<Scope>(e).unwrap()).collect::<Vec<_>>();
    let before = all(&g);
    g.choose("calib", "cad");
    assert_eq!(all(&g), before, "the layout of the graph doesn't jump when you decide");
}

// ---------- views ----------

#[test]
fn collapsing_folds_scopes_and_redraws_links_to_what_is_visible() {
    let mut g = estimated();
    g.task("other").depends("other", "export");
    let full = ViewGraph::of(g.g());

    let top = full.collapse(g.g(), &BTreeSet::new());
    assert_eq!(g.entity_names(top.nodes.keys().copied()), vec!["calib", "other"]);
    assert_eq!(top.nodes[&g.id("calib")].hidden, 7, "cad, free, the gap and four tasks");
    assert!(top.edges[&(g.id("other"), g.id("calib"))].kinds.contains(DEPENDS_ON), "a link into the fold points at the fold");

    let one = full.collapse(g.g(), &BTreeSet::from([g.id("calib")]));
    assert!(one.nodes.contains_key(&g.id("cad")) && one.nodes.contains_key(&g.id("static")) && !one.nodes.contains_key(&g.id("export")));
    assert_eq!(one.nodes[&g.id("cad")].hidden, 1);
    assert!(one.edges.contains_key(&(g.id("other"), g.id("cad"))), "it follows the fold in");
}

#[test]
fn within_shows_a_scope_and_its_boundary() {
    let mut g = estimated();
    g.task("other").depends("other", "export");
    let v = ViewGraph::of(g.g()).within(g.g(), g.id("cad"));
    assert_eq!(g.entity_names(v.nodes.iter().filter(|(_, n)| !n.external).map(|(k, _)| *k)), vec!["cad", "export"]);
    assert!(v.nodes[&g.id("other")].external, "a link reaches in from outside, so it shows, marked");
    assert!(v.nodes[&g.id("calib")].external);
    assert!(v.nodes[&g.id("static")].external, "shared work lives above cad, and is visible from it");
}

#[test]
fn transforms_compose_including_with_what_decisions_rule_out() {
    let mut g = estimated();
    g.choose("calib", "cad");
    let v = ViewGraph::of(g.g()).retain(|e| !g.g().get::<Need>(e).unwrap().unwrap().0.is_never());
    assert!(!v.nodes.contains_key(&g.id("search")) && !v.nodes.contains_key(&g.id("free")));
    let folded = v.collapse(g.g(), &BTreeSet::new());
    assert_eq!(folded.nodes[&g.id("calib")].hidden, v.nodes.len() - 1);
}

// ---------- scale ----------

#[test]
fn it_scales_with_structure_not_with_the_number_of_worlds() {
    let mut g = G::new();
    let mut ops = g.make("root");
    let root = g.id("root");
    for i in 0..40 {
        let (dn, an, bn) = (format!("d{i}"), format!("d{i}a"), format!("d{i}b"));
        for n in [&dn, &an, &bn] {
            ops.extend(g.make(n));
        }
        let (dec, a, b) = (g.id(&dn), g.id(&an), g.id(&bn));
        ops.extend([Op::add(dec, DECISION), Op::add(a, TASK), Op::add(b, TASK)]);
        ops.extend([Op::link(dec, root, PART_OF), Op::link(a, dec, ALTERNATIVE), Op::link(b, dec, ALTERNATIVE)]);
        for j in 0..20 {
            let tn = format!("t{i}_{j}");
            ops.extend(g.make(&tn));
            let t = g.id(&tn);
            ops.extend([Op::add(t, TASK), Op::link(t, a, PART_OF)]);
        }
    }
    g.commit(ops);
    assert_eq!(g.need("t3_0"), "d3=d3a");
    assert!(g.g().get::<Done>(root).unwrap().unwrap().finished().prob().abs() < 1e-9);
}

#[test]
fn a_long_chain_computes() {
    let mut g = G::new();
    let mut ops = g.make("n0");
    let mut last = g.id("n0");
    for i in 1..20_000 {
        let name = format!("n{i}");
        ops.extend(g.make(&name));
        let next = g.id(&name);
        ops.push(Op::link(next, last, PART_OF));
        last = next;
    }
    g.commit(ops);
    assert!(g.need_of("n19999").is_always());
    assert_eq!(g.g().get::<ScopeDepth>(last), Ok(Some(ScopeDepth(19_999))));
    assert_eq!(g.g().get::<Done>(g.id("n0")).unwrap().unwrap().0.0, Space::global().pure(None));
}
