//! Each scenario states what the spec says should happen. See docs/kernel-spec.md.

use recollect_kernel::formula::{FALSE, TRUE};
use recollect_kernel::log::{Log, choose, close_gap, mark_incomplete, new_decision, reopen, set, unset};
use recollect_kernel::store::*;
use recollect_kernel::systems::*;
use serde_json::{Value, json};

/// A small builder over the log, so scenarios go through the real write path.
struct G(Log);

impl G {
    fn new() -> Self {
        G(Log::default())
    }
    fn commit(&mut self, ops: Vec<Op>) {
        self.0.commit("test", "setup", ops).expect("valid change");
    }
    fn node(&mut self, id: &str, comps: &[(&str, Value)]) -> &mut Self {
        let mut ops = vec![set(e(id), "title", json!(id))];
        ops.extend(comps.iter().map(|(c, v)| set(e(id), c, v.clone())));
        self.commit(ops);
        self
    }
    fn task(&mut self, id: &str) -> &mut Self {
        self.node(id, &[(TASK, json!({}))])
    }
    fn decision(&mut self, id: &str) -> &mut Self {
        self.node(id, &[(DECISION, json!({}))])
    }
    fn link(&mut self, from: &str, to: &str, c: &str, v: Value) -> &mut Self {
        self.commit(vec![set(l(from, to), c, v)]);
        self
    }
    fn part_of(&mut self, from: &str, to: &str) -> &mut Self {
        self.link(from, to, PART_OF, json!({}))
    }
    fn option(&mut self, from: &str, to: &str) -> &mut Self {
        self.link(from, to, ALTERNATIVE, json!({}))
    }
    fn done(&mut self, id: &str) -> &mut Self {
        self.commit(vec![set(e(id), DONE, json!({}))]);
        self
    }
    fn d(&self) -> Derived {
        compute(&self.0.store)
    }
}

/// The calibration example: three approaches, overlapping work.
fn calibration() -> G {
    let mut g = G::new();
    g.decision("calib");
    for o in ["cad", "board", "free"] {
        g.node(o, &[]).option(o, "calib");
    }
    for t in ["imu", "residual"] {
        g.task(t).part_of(t, "calib");
    }
    g.task("static").part_of("static", "cad").part_of("static", "free");
    g.task("assoc").part_of("assoc", "cad").part_of("assoc", "free").link("assoc", "static", DEPENDS_ON, json!({}));
    g.task("export").part_of("export", "cad");
    g.task("detector").part_of("detector", "board");
    g.task("search").part_of("search", "free").link("search", "assoc", DEPENDS_ON, json!({}));
    g
}

// ---------- needed ----------

#[test]
fn s01_needed_is_exact() {
    let g = calibration();
    let d = g.d();
    assert!(d.holds("need", "imu"), "every option needs it");
    assert_eq!(d.show("need", "static"), "calib∈{cad,free}");
    assert_eq!(d.show("need", "detector"), "calib=board");
    assert!((d.prob("need", "static") - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(d.get("need", "calib"), TRUE);
}

#[test]
fn s02_safe_work_and_pressure() {
    let g = calibration();
    let d = g.d();
    assert_eq!(d.ids_where("actionable", |f| f == TRUE), vec!["imu", "residual"]);
    assert_eq!(d.show("actionable", "assoc"), "never", "it waits for static in every world that needs it");
    assert_eq!(pressure(&g.0.store, &d, "calib"), Pressure::Free(vec!["imu".into(), "residual".into()]));

    let mut g = calibration();
    g.done("imu").done("residual");
    let d = g.d();
    assert!(matches!(pressure(&g.0.store, &d, "calib"), Pressure::Blocking(_)), "only option-specific work is left");
}

#[test]
fn s03_correlated_paths_are_exact() {
    // The bug in the current app: (root=a ∧ x=p) ∨ root=b is 75%, not 62.5%.
    let mut g = G::new();
    g.decision("root").node("a", &[]).option("a", "root").node("b", &[]).option("b", "root");
    g.decision("x").part_of("x", "a").node("p", &[]).option("p", "x").node("q", &[]).option("q", "x");
    g.task("y").part_of("y", "p").part_of("y", "b");
    let d = g.d();
    assert!((d.prob("need", "y") - 0.75).abs() < 1e-9, "{}", d.prob("need", "y"));
    assert_eq!(d.show("need", "y"), "root=a ∧ x=p ∨ root=b");
}

#[test]
fn s04_ruling_out_narrows_the_worlds() {
    let mut g = calibration();
    g.commit(vec![set(l("board", "calib"), ALTERNATIVE, json!({ "ruled_out": { } }))]);
    let d = g.d();
    assert!(d.holds("need", "static"), "cad and free both need it, and they're all that's left");
    assert_eq!(d.get("need", "detector"), FALSE, "pruned, with nothing stored on detector");
    assert!(!g.0.store.has("detector", DROPPED));
}

// ---------- the bug that started this: reopening ----------

#[test]
fn s05_deciding_and_reopening_never_touch_the_children() {
    let mut g = calibration();
    let children_before = g.0.store.entities["detector"].clone();
    let links_before = g.0.store.links.clone();
    let d0 = g.d();

    g.commit(vec![choose("calib", "cad")]);
    let d1 = g.d();
    assert_eq!(d1.get("need", "detector"), FALSE);
    assert!(d1.holds("need", "export"));

    g.commit(vec![reopen("calib")]);
    let d2 = g.d();
    assert_eq!(d2.show("need", "detector"), d0.show("need", "detector"), "everything comes back on its own");
    assert_eq!(g.0.store.entities["detector"], children_before);
    assert_eq!(g.0.store.links, links_before, "no link changed at any point");

    g.commit(vec![choose("calib", "board")]);
    let d3 = g.d();
    assert!(d3.holds("need", "detector"));
    assert_eq!(d3.get("need", "export"), FALSE);
}

// ---------- completion, including your question ----------

#[test]
fn s06_completion_is_choice_aware() {
    let mut g = calibration();
    g.done("imu").done("residual");
    assert_eq!(g.d().get("done", "calib"), FALSE, "no option is complete yet");

    // Finish one approach: done if you pick it. Not done regardless.
    g.done("static").done("assoc").done("export");
    let d = g.d();
    assert_eq!(d.show("done", "cad"), "always");
    assert_eq!(d.show("done", "calib"), "calib=cad");
    assert!(!d.holds("done", "calib") && d.possible("done", "calib"));

    // Finish the others too: done in every world, without having decided.
    g.done("detector").done("search");
    let d = g.d();
    assert!(d.holds("done", "board"));
    assert!(d.holds("done", "calib"), "complete whichever way it goes");
    assert!(g.0.store.chosen("calib").is_none());
}

#[test]
fn s07_all_options_done_means_done_without_deciding() {
    let mut g = G::new();
    g.decision("d");
    for o in ["a", "b"] {
        g.task(o).option(o, "d");
    }
    assert_eq!(g.d().get("done", "d"), FALSE);
    g.done("a");
    assert_eq!(g.d().show("done", "d"), "d=a");
    g.done("b");
    assert!(g.d().holds("done", "d"), "complete whichever way it's decided");
    assert!(g.0.store.chosen("d").is_none(), "and still undecided, which is a separate fact");
}

#[test]
fn s08_dependency_says_what_it_waits_for() {
    let mut g = G::new();
    g.decision("d");
    for o in ["a", "b"] {
        g.task(o).option(o, "d");
    }
    g.task("after_work").link("after_work", "d", DEPENDS_ON, json!({}));
    g.task("after_choice").link("after_choice", "d", DEPENDS_ON, json!({ "on": "decided" }));

    let d = g.d();
    assert!(d.holds("blocked", "after_work") && d.holds("blocked", "after_choice"));

    // Finish option a: the work could be complete, depending on the decision.
    g.done("a");
    let d = g.d();
    assert_eq!(d.show("blocked", "after_work"), "d=b", "blocked unless you choose a: exactly what would unblock it");
    assert!(d.holds("blocked", "after_choice"));

    // Decide b instead: the choice dependency clears, the work one doesn't.
    g.commit(vec![choose("d", "b")]);
    let d = g.d();
    assert_eq!(d.get("blocked", "after_choice"), FALSE);
    assert!(d.holds("blocked", "after_work"), "b isn't done");
}

#[test]
fn s09_a_decision_that_is_only_a_decision() {
    // No work under the options at all: "which colour?". Options are nodes like any other, and
    // a node with nothing required of it is complete, so what keeps this open is the gap.
    let mut g = G::new();
    g.commit(new_decision("colour"));
    for o in ["red", "blue"] {
        g.node(o, &[]).option(o, "colour");
    }
    g.task("paint").link("paint", "colour", DEPENDS_ON, json!({}));
    g.task("mix").link("mix", "colour", DEPENDS_ON, json!({ "on": "decided" }));
    let d = g.d();
    assert_eq!(d.show("done", "colour"), "colour∈{blue,red}", "complete unless it's something nobody has named");
    assert_eq!(d.show("blocked", "paint"), "colour=colour.unknown");
    assert!(d.holds("blocked", "mix"));

    // Closing the gap says these are all the options. Every one is complete, so it's done
    // without being decided, as with any other choice, and only a dependent that waits for the
    // answer still waits.
    g.commit(close_gap("colour"));
    let d = g.d();
    assert!(d.holds("done", "colour"));
    assert_eq!(d.get("blocked", "paint"), FALSE);
    assert!(d.holds("blocked", "mix"));

    g.commit(vec![choose("colour", "red")]);
    let d = g.d();
    assert!(d.holds("done", "colour"), "{}", d.show("done", "colour"));
    assert_eq!(d.get("blocked", "mix"), FALSE);
}

#[test]
fn s10_stated_done_and_rollup_can_disagree_visibly() {
    let mut g = G::new();
    g.node("goal", &[("goal", json!({}))]);
    g.task("t").part_of("t", "goal");
    g.done("goal");
    let d = g.d();
    assert!(d.holds("done", "goal"), "a statement is a fact");
    assert!(d.notices.iter().any(|n| n.rule == "stated_done_with_open_work" && n.id == "goal"), "{:?}", d.notices);
}

// ---------- dependencies ----------

#[test]
fn s11_dependency_on_work_that_is_not_needed_does_not_block() {
    let mut g = calibration();
    g.link("export", "detector", DEPENDS_ON, json!({}));
    // export is needed if cad; detector is only needed if board. They can't both be needed.
    let d = g.d();
    assert_eq!(d.show("blocked", "export"), "calib=board");
    assert_eq!(d.show("actionable", "export"), "calib=cad", "where it's needed, it isn't blocked");
}

#[test]
fn s12_dependencies_are_inherited_from_ancestors() {
    let mut g = G::new();
    g.task("prereq").node("phase", &[]).task("step");
    g.part_of("step", "phase").link("phase", "prereq", DEPENDS_ON, json!({}));
    assert!(g.d().holds("blocked", "step"));
    g.done("prereq");
    assert_eq!(g.d().get("blocked", "step"), FALSE);
}

#[test]
fn s13_deadlocks_are_refused() {
    // Depending on your own ancestor.
    let mut g = G::new();
    g.task("parent").task("child").part_of("child", "parent");
    let err = g.0.commit("t", "x", vec![set(l("child", "parent"), DEPENDS_ON, json!({}))]);
    // That's one link holding two components, which is fine in itself, but it can't complete.
    assert!(err.is_err_and(|e| e[0].rule == "no_deadlock"));

    // Two phases whose steps wait on each other across the hierarchy.
    let mut g = G::new();
    g.node("a", &[]).node("b", &[]).task("a1").task("b1").part_of("a1", "a").part_of("b1", "b");
    g.link("a", "b", DEPENDS_ON, json!({}));
    let err = g.0.commit("t", "x", vec![set(l("b1", "a1"), DEPENDS_ON, json!({}))]);
    assert!(err.is_err_and(|e| e[0].rule == "no_deadlock"), "a1 waits for b, which needs b1, which waits for a1");
}

#[test]
fn s14_invariants_refuse_and_change_nothing() {
    let mut g = calibration();
    let before = g.0.store.clone();
    let events = g.0.events.len();
    assert!(g.0.commit("t", "x", vec![choose("calib", "nonsense")]).is_err());
    assert!(g.0.commit("t", "x", vec![choose("calib", "imu")]).is_err(), "imu isn't one of its options");
    assert_eq!(g.0.store, before);
    assert_eq!(g.0.events.len(), events, "refused changes leave no event");

    g.commit(vec![choose("calib", "cad")]);
    let refused = g.0.commit("t", "x", vec![set(l("cad", "calib"), ALTERNATIVE, json!({ "ruled_out": {} }))]);
    assert!(refused.is_err_and(|e| e[0].rule == "chosen_must_be_a_live_option"));
}

// ---------- dropped: a deliberate statement about a node ----------

#[test]
fn s15_dropping_is_explicit_and_reversible() {
    let mut g = G::new();
    g.task("parent_needs");
    g.node("phase", &[]).task("a").task("b").part_of("a", "phase").part_of("b", "phase");
    g.done("a");
    assert!(!g.d().holds("done", "phase"));
    g.commit(vec![set(e("b"), DROPPED, json!({}))]);
    let d = g.d();
    assert!(d.holds("done", "phase"), "it no longer requires b");
    assert!(d.notices.iter().any(|n| n.rule == "dropped_part_of_live_work" && n.id == "b"), "so that isn't silent");
    g.commit(vec![unset(e("b"), DROPPED)]);
    assert!(!g.d().holds("done", "phase"));
}

// ---------- audit ----------

#[test]
fn s16_the_log_answers_who_when_why() {
    let mut g = calibration();
    let seq = g.0.commit("steven", "CAD model is already accurate", vec![choose("calib", "cad")]).unwrap();
    let ev = g.0.blame(&e("calib"), DECISION).unwrap();
    assert_eq!((ev.seq, ev.author.as_str(), ev.why.as_str()), (seq, "steven", "CAD model is already accurate"));

    // Replay reproduces the store exactly, at any point.
    assert_eq!(g.0.state_at(seq), g.0.store);
    assert_eq!(g.0.state_at(seq - 1).chosen("calib"), None);
    assert_eq!(compute(&g.0.state_at(seq - 1)).show("need", "detector"), "calib=board", "derived values are reproducible at any point in history");

    // Undo appends; history isn't rewritten.
    let u = g.0.undo(seq, "steven", "changed my mind").unwrap();
    assert_eq!(g.0.store.chosen("calib"), None);
    assert_eq!(g.0.state_at(u - 1).chosen("calib"), Some("cad"));
    assert_eq!(g.0.history(&e("calib")).iter().filter(|e| e.seq >= seq).count(), 2);
}

#[test]
fn s17_derived_values_are_a_pure_function_of_the_store() {
    let g = calibration();
    let (a, b) = (g.d(), g.d());
    for cell in ["need", "done", "blocked", "actionable"] {
        for id in g.0.store.entities.keys() {
            assert_eq!(a.show(cell, id), b.show(cell, id));
        }
    }
}

// ---------- pluggable systems: new uses without touching the kernel ----------

/// A use the kernel knows nothing about: which work is exposed to a risk. A `threatens` link
/// from a risk node, and exposure = threatened ∧ the risk is still open.
struct Exposed;
impl Cell for Exposed {
    const NAME: &'static str = "exposed";
    type Value = recollect_kernel::formula::F;
    fn eval(cx: &Cx, id: &str) -> Self::Value {
        let open_risk = cx.links_into_raw(id, "threatens").into_iter().any(|(risk, _)| cx.live(risk) && cx.raw(risk, DONE).is_none());
        if open_risk { cx.cell::<Need>(id) } else { FALSE }
    }
}

#[test]
fn s18_a_new_use_is_a_new_cell_and_a_new_component() {
    let mut g = calibration();
    g.node("diverges", &[("risk", json!({}))]);
    g.link("diverges", "search", "threatens", json!({}));
    let mut systems = kernel();
    systems.add::<Exposed>();
    let d = run(&g.0.store, &systems);
    assert_eq!(d.show("exposed", "search"), "calib=free", "only exposed if you take the option that needs it");
    // Nothing declared that it reads need: the engine saw it happen.
    assert!(d.trace[&("exposed".to_string(), "search".to_string())].contains(&Dep::Cell("need".into(), "search".into())));
    // The new components changed nothing the kernel computes.
    assert_eq!(d.show("need", "search"), calibration().d().show("need", "search"));
}

struct LoopA;
impl Cell for LoopA {
    const NAME: &'static str = "loop_a";
    type Value = recollect_kernel::formula::F;
    fn eval(cx: &Cx, id: &str) -> Self::Value {
        cx.cell::<LoopB>(id)
    }
}

struct LoopB;
impl Cell for LoopB {
    const NAME: &'static str = "loop_b";
    type Value = recollect_kernel::formula::F;
    fn eval(cx: &Cx, id: &str) -> Self::Value {
        cx.cell::<LoopA>(id)
    }
}

#[test]
fn s19_cell_cycles_are_reported_not_fatal() {
    let mut g = G::new();
    g.task("t");
    let mut systems = kernel();
    systems.add::<LoopA>().add::<LoopB>();
    let d = run(&g.0.store, &systems);
    assert!(d.cycles.iter().any(|c| c.iter().any(|(name, _)| name == "loop_a")), "{:?}", d.cycles);
    assert!(d.holds("need", "t"), "and everything else still computes");
}

// ---------- second round: edges of the semantics ----------

#[test]
fn s21_an_option_with_no_work_next_to_a_finished_one() {
    // Both are complete, so the decision is complete whichever way it goes.
    let mut g = G::new();
    g.decision("d").node("a", &[]).option("a", "d").task("b").option("b", "d").done("b");
    assert!(g.d().holds("done", "d"));
    assert_eq!(g.0.store.chosen("d"), None);
    // An option that isn't thought through yet says so.
    g.commit(mark_incomplete("a"));
    assert_eq!(g.d().show("done", "d"), "d=b");
    g.commit(vec![choose("d", "a")]);
    assert_eq!(g.d().get("done", "d"), FALSE, "choosing it doesn't specify it");
    g.commit(close_gap("a"));
    assert!(g.d().holds("done", "d"));
}

#[test]
fn s22_nested_decisions_only_complete_when_their_chosen_path_does() {
    let mut g = G::new();
    g.decision("outer").node("p", &[]).option("p", "outer").node("q", &[]).option("q", "outer");
    g.decision("inner").part_of("inner", "p");
    g.task("x").option("x", "inner").task("y").option("y", "inner");
    g.task("z").part_of("z", "q");
    let d = g.d();
    assert_eq!(d.show("need", "x"), "outer=p ∧ inner=x");
    g.done("x");
    assert_eq!(g.d().show("done", "outer"), "outer=p ∧ inner=x");
    g.commit(vec![choose("inner", "x")]);
    assert_eq!(g.d().show("done", "outer"), "outer=p", "the inner decision is settled, the outer isn't");
    g.commit(vec![choose("outer", "q")]);
    assert_eq!(g.d().get("done", "outer"), FALSE, "the chosen path has undone work");
    assert_eq!(g.d().get("need", "x"), FALSE, "and the other path's work is no longer needed");
}

#[test]
fn s23_shared_work_survives_any_path_that_still_wants_it() {
    let mut g = calibration();
    g.commit(vec![set(l("free", "calib"), ALTERNATIVE, json!({ "ruled_out": {} }))]);
    assert_eq!(g.d().show("need", "static"), "calib=cad", "free is gone, cad still needs it");
    g.commit(vec![set(l("cad", "calib"), ALTERNATIVE, json!({ "ruled_out": {} }))]);
    assert_eq!(g.d().get("need", "static"), FALSE);
    // Only board left: it's forced, in every remaining world, though nobody has chosen it.
    assert!(g.d().holds("need", "detector"));
    assert_eq!(g.0.store.chosen("calib"), None);
    // And it all reverses.
    g.commit(vec![set(l("cad", "calib"), ALTERNATIVE, json!({}))]);
    assert_eq!(g.d().show("need", "static"), "calib=cad");
}

#[test]
fn s24_no_options_left_is_visible() {
    let mut g = G::new();
    g.decision("d").node("a", &[]).option("a", "d");
    g.commit(vec![set(e("a"), DROPPED, json!({}))]);
    let d = g.d();
    assert!(d.notices.iter().any(|n| n.rule == "decision_without_options"));
    assert_eq!(d.get("done", "d"), FALSE, "it can't complete with nothing to choose");
}

#[test]
fn s25_dropping_a_branch_prunes_below_it_except_shared_work() {
    let mut g = G::new();
    g.node("root", &[("goal", json!({}))]);
    g.node("a", &[]).part_of("a", "root").node("b", &[]).part_of("b", "root");
    g.task("only_a").part_of("only_a", "a").task("both").part_of("both", "a").part_of("both", "b");
    g.commit(vec![set(e("a"), DROPPED, json!({}))]);
    let d = g.d();
    assert_eq!(d.get("need", "only_a"), FALSE);
    assert!(d.holds("need", "both"));
}

#[test]
fn s26_a_dependency_only_waits_for_what_something_requires() {
    let mut g = G::new();
    g.node("note", &[]).task("t").link("t", "note", DEPENDS_ON, json!({}));
    assert_eq!(g.d().get("blocked", "t"), FALSE, "a note requires nothing");
    g.commit(mark_incomplete("note"));
    assert!(g.d().holds("blocked", "t"), "until it says there's more to come");
    g.commit(close_gap("note"));
    assert_eq!(g.d().get("blocked", "t"), FALSE);
}

#[test]
fn s27_a_link_with_no_components_is_an_association() {
    let mut g = G::new();
    g.node("a", &[]).node("b", &[]).link("a", "b", "note", json!("related"));
    g.commit(vec![unset(l("a", "b"), "note")]);
    assert!(g.0.store.links.contains_key(&("a".to_string(), "b".to_string())), "the link itself remains");
    assert!(g.d().holds("need", "a"), "and it means nothing structurally");
    g.link("a", "b", PART_OF, json!({})).link("a", "b", "supports", json!({ "strength": 2 }));
    assert_eq!(g.0.store.links[&("a".to_string(), "b".to_string())].len(), 2, "one link, several components");
}

#[test]
fn s28_external_edits_are_audited_not_lost() {
    let mut g = calibration();
    let mut disk = g.0.store.clone();
    disk.entities.get_mut("imu").unwrap().insert(DONE.into(), json!({}));
    disk.links.remove(&("search".to_string(), "assoc".to_string()));
    disk.links.remove(&("search".to_string(), "free".to_string()));
    disk.entities.remove("search");
    let issues = g.0.reconcile(&disk, "git pull");
    assert!(issues.is_empty());
    assert_eq!(g.0.store, disk, "the store now matches the files");
    let ev = g.0.blame(&e("imu"), DONE).unwrap();
    assert_eq!((ev.author.as_str(), ev.why.as_str()), ("git pull", "external edit"));
    let seq = ev.seq;
    assert_eq!(g.0.state_at(seq), disk, "and replay still reproduces it");
    assert!(g.0.state_at(seq - 1).entities.contains_key("search"));
    g.0.undo(seq, "steven", "that pull was wrong").unwrap();
    assert!(g.0.store.entities.contains_key("search"), "external edits can be undone too");
    assert!(g.0.store.links.contains_key(&("search".to_string(), "assoc".to_string())));
    assert_eq!(g.0.reconcile(&g.0.store.clone(), "noop"), vec![], "no change, no event");
}

#[test]
fn s29_an_external_edit_that_breaks_a_rule_is_recorded_and_reported() {
    let mut g = calibration();
    let events = g.0.events.len();
    let mut disk = g.0.store.clone();
    disk.links.entry(("imu".to_string(), "imu".to_string())).or_default();
    let issues = g.0.reconcile(&disk, "hand edit");
    assert_eq!(issues[0].rule, "no_self_links");
    assert_eq!(g.0.events.len(), events + 1);
    // Unrelated work can continue, and the fix is an ordinary change.
    g.commit(vec![set(e("residual"), DONE, json!({}))]);
    g.commit(vec![Op::Remove(l("imu", "imu"))]);
    assert!(g.0.store.check().is_empty());
}

// ---------- third round: found by running on the real demo graph ----------

#[test]
fn s30_a_done_leaf_is_not_a_contradiction() {
    let mut g = G::new();
    g.task("t").done("t");
    let d = g.d();
    assert!(d.notices.is_empty(), "the statement is all a leaf task has: {:?}", d.notices);
    assert!(d.holds("done", "t"));
    assert_eq!(d.get("all_required_done", "t"), FALSE, "the two columns stay distinct");
}

#[test]
fn s31_stated_and_derived_are_separate_cells() {
    let mut g = calibration();
    g.commit(vec![set(e("calib"), DONE, json!({}))]);
    let d = g.d();
    assert!(d.holds("done", "calib"));
    assert_eq!(d.get("all_required_done", "calib"), FALSE);
    let notice = d.notices.iter().find(|n| n.id == "calib").unwrap();
    assert_eq!(notice.rule, "stated_done_with_open_work");
}

#[test]
fn s32_blockers_are_named_with_their_conditions() {
    let mut g = calibration();
    g.link("export", "detector", DEPENDS_ON, json!({}));
    let d = g.d();
    let b = blockers(&d, "search");
    assert_eq!(b.iter().map(|(id, f)| (id.as_str(), d.mdd.show(*f))).collect::<Vec<_>>(), vec![("assoc", "calib∈{cad,free}".to_string())]);
    let b = blockers(&d, "export");
    assert_eq!(b.iter().map(|(id, f)| (id.as_str(), d.mdd.show(*f))).collect::<Vec<_>>(), vec![("detector", "calib=board".to_string())]);
    // Inherited: a child waits for what its parent waits for, and says so.
    let mut g = G::new();
    g.task("prereq").node("phase", &[]).task("step").part_of("step", "phase").link("phase", "prereq", DEPENDS_ON, json!({}));
    let d = g.d();
    assert_eq!(blockers(&d, "step")[0].0, "prereq");
}

#[test]
fn s33_pressure_only_applies_while_open() {
    let mut g = calibration();
    g.commit(vec![choose("calib", "cad")]);
    let d = g.d();
    assert_eq!(pressure(&g.0.store, &d, "calib"), Pressure::Decided);
}

#[test]
fn s34_what_if_and_commit_share_one_code_path() {
    let mut g = calibration();
    let before = g.0.store.clone();
    let events = g.0.events.len();

    // The option matrix is just each option's hypothetical.
    let cad = what_if(&g.0.store, &[choose("calib", "cad")]).unwrap();
    let board = what_if(&g.0.store, &[choose("calib", "board")]).unwrap();
    assert!(cad.holds("need", "export") && !cad.possible("need", "detector"));
    assert!(board.holds("need", "detector") && !board.possible("need", "export"));
    assert_eq!(g.0.store, before, "previewing writes nothing");
    assert_eq!(g.0.events.len(), events, "and logs nothing");

    // The consequences a confirm step would show are a comparison of two derivations.
    let now = g.d();
    let became_safe: Vec<_> = cad.ids_where("actionable", |f| f == TRUE).into_iter().filter(|id| !now.holds("actionable", id)).map(String::from).collect();
    assert_eq!(became_safe, vec!["export", "static"]);

    // A preview agrees exactly with really doing it.
    g.commit(vec![choose("calib", "cad")]);
    let real = g.d();
    for id in g.0.store.entities.keys() {
        assert_eq!(real.show("actionable", id), cad.show("actionable", id));
    }

    // An impossible hypothetical is refused the same way a real change would be.
    assert!(what_if(&g.0.store, &[choose("calib", "imu")]).is_err());
}

// ---------- scale ----------

#[test]
fn s20_scales_with_structure_not_world_count() {
    // 40 independent binary decisions is 2^40 worlds: enumeration would never finish.
    let mut g = G::new();
    let mut ops = vec![set(e("root"), "title", json!("root"))];
    for i in 0..40 {
        let (dec, a, b) = (format!("d{i}"), format!("d{i}a"), format!("d{i}b"));
        ops.push(set(e(&dec), DECISION, json!({})));
        ops.push(set(e(&a), TASK, json!({})));
        ops.push(set(e(&b), TASK, json!({})));
        ops.push(set(l(&dec, "root"), PART_OF, json!({})));
        ops.push(set(l(&a, &dec), ALTERNATIVE, json!({})));
        ops.push(set(l(&b, &dec), ALTERNATIVE, json!({})));
        for j in 0..20 {
            let t = format!("t{i}_{j}");
            ops.push(set(e(&t), TASK, json!({})));
            ops.push(set(l(&t, &a), PART_OF, json!({})));
        }
    }
    g.commit(ops);
    let start = std::time::Instant::now();
    let d = g.d();
    let took = start.elapsed();
    assert_eq!(d.show("need", "t3_0"), "d3=d3a");
    assert!((d.prob("done", "root")).abs() < 1e-9);
    println!("{} entities, 2^40 worlds: {took:?}", g.0.store.entities.len());
    assert!(took.as_secs() < 5, "took {took:?}");
}
