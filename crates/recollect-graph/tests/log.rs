//! The log and invariants: who changed what and why, what gets refused, and what happens when the
//! files change underneath it. Crashes are in `tests/crash.rs`.

use std::sync::Arc;

use recollect_systems::plan::{self, Alternative, Decision, DependsOn, Done, Dropped, Estimate, PartOf, Task};
use recollect_graph::{Component, Compute, Cx, Entity, Error, Graph, LoadProblem, Log, Name, Op, OpenError, Place, Registry, Seen, SavedGraph, Target, component, rule};
use serde_json::{Value, json};

fn plan_registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    Arc::new(reg)
}

fn plan_log() -> Log {
    Log::new(plan_registry())
}

fn refused(log: &mut Log, ops: Vec<Op>) -> Vec<String> {
    let err = log.commit("t", "x", ops).expect_err("should be refused");
    err.violations().iter().map(|v| v.rule.clone()).collect()
}

fn ok(log: &mut Log, ops: Vec<Op>) -> u64 {
    log.commit("t", "setup", ops).expect("a valid change")
}

/// A decision with two options, one with a task under it.
struct Calib {
    calib: Entity,
    cad: Entity,
    free: Entity,
    imu: Entity,
}

fn calibration(log: &mut Log) -> Calib {
    let (calib, cad, free, imu) = (Entity::new(), Entity::new(), Entity::new(), Entity::new());
    ok(log, vec![Op::set(calib, Decision::default()), Op::set(cad, Name("cad".into())), Op::set(free, Name("free".into())), Op::link(cad, calib, Alternative::default()), Op::link(free, calib, Alternative::default())]);
    ok(log, vec![Op::set(imu, Task {}), Op::set(imu, Estimate::days(1.0)), Op::link(imu, cad, PartOf)]);
    Calib { calib, cad, free, imu }
}

fn choose(decision: Entity, option: Entity) -> Op {
    Op::set(decision, Decision { chosen: Some(option) })
}

// ---------- the log answers who, when and why ----------

#[test]
fn the_log_answers_who_when_why() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let seq = log.commit("steven", "CAD model is already accurate", vec![choose(c.calib, c.cad)]).unwrap();

    let ev = log.blame(Target::Node(c.calib), Decision::NAME).unwrap();
    assert_eq!((ev.seq, ev.author.as_str(), ev.why.as_str()), (seq, "steven", "CAD model is already accurate"));
    assert!(ev.at > 0, "and when");

    // Replay reproduces the graph exactly, at any point.
    assert_eq!(log.state_at(seq).save().unwrap(), log.graph().save().unwrap());
    assert_eq!(log.state_at(seq - 1).get::<Decision>(c.calib), Ok(Some(Decision { chosen: None })));

    // Undo appends: history isn't rewritten.
    let u = log.undo(seq, "steven", "changed my mind").unwrap();
    assert_eq!(log.graph().get::<Decision>(c.calib), Ok(Some(Decision { chosen: None })));
    assert_eq!(log.state_at(u - 1).get::<Decision>(c.calib), Ok(Some(Decision { chosen: Some(c.cad) })));
    assert_eq!(log.history(Target::Node(c.calib)).iter().filter(|e| e.seq >= seq).count(), 2);
    assert!(log.blame(Target::Node(c.free), "nothing").is_none());
}

#[test]
fn undoing_a_creation_removes_it_rather_than_leaving_it_empty() {
    let mut log = plan_log();
    let (a, b) = (Entity::new(), Entity::new());
    let s = ok(&mut log, vec![Op::set(a, Task {}), Op::Spawn { id: b }, Op::link(a, b, PartOf)]);
    log.undo(s, "t", "undo").unwrap();
    assert_eq!(log.graph().save().unwrap(), SavedGraph::default());
}

#[test]
fn every_kind_of_op_survives_being_written_down() {
    let (a, b) = (Entity::new(), Entity::new());
    let ops = vec![
        Op::Spawn { id: a },
        Op::Despawn { id: a },
        Op::set(a, Done::yes()),
        Op::set_runtime(a, "note", json!({ "text": null })),
        Op::set_rule(a, "x", "some/rule", json!({ "n": 1 })),
        Op::remove::<Done>(a),
        // A unit component's value is null, which isn't the same as no value.
        Op::link(a, b, PartOf),
        Op::unlink::<PartOf>(a, b),
        Op::Default { component: "x".into(), to: None },
    ];
    let text = serde_json::to_string(&ops).unwrap();
    assert_eq!(serde_json::from_str::<Vec<Op>>(&text).unwrap(), ops, "{text}");
}

// ---------- invariants ----------

#[test]
fn deadlocks_are_refused() {
    // Depending on your own ancestor.
    let mut log = plan_log();
    let (parent, child) = (Entity::new(), Entity::new());
    ok(&mut log, vec![Op::set(parent, Task {}), Op::set(child, Task {}), Op::link(child, parent, PartOf)]);
    let rules = refused(&mut log, vec![Op::link(child, parent, DependsOn::default())]);
    assert!(rules.iter().all(|r| r == "no_deadlock"), "{rules:?}");

    // Two phases whose steps wait on each other across the hierarchy.
    let mut log = plan_log();
    let (a, b, a1, b1) = (Entity::new(), Entity::new(), Entity::new(), Entity::new());
    ok(&mut log, vec![Op::Spawn { id: a }, Op::Spawn { id: b }, Op::Spawn { id: a1 }, Op::Spawn { id: b1 }, Op::link(a1, a, PartOf), Op::link(b1, b, PartOf)]);
    ok(&mut log, vec![Op::link(a, b, DependsOn::default())]);
    let rules = refused(&mut log, vec![Op::link(b1, a1, DependsOn::default())]);
    assert!(rules.contains(&"no_deadlock".to_string()), "a1 waits for b, which needs b1, which waits for a1: {rules:?}");
    assert!(log.violations().is_empty(), "and it's as if it never happened");
}

#[test]
fn a_refused_commit_changes_nothing_and_leaves_no_event() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let before = log.graph().save().unwrap();
    let events = log.events().len();

    assert_eq!(refused(&mut log, vec![choose(c.calib, c.imu)]), ["chosen_must_be_a_live_option"], "imu isn't one of its options");
    assert_eq!(refused(&mut log, vec![choose(c.calib, Entity::new())]), ["chosen_must_be_a_live_option"]);
    assert_eq!(log.graph().save().unwrap(), before);
    assert_eq!(log.events().len(), events);

    // A change that's partly fine is refused as a whole, including what came before the bad part.
    let extra = Entity::new();
    assert_eq!(refused(&mut log, vec![Op::set(extra, Done::yes()), choose(c.calib, c.imu)]), ["chosen_must_be_a_live_option"]);
    assert_eq!(refused(&mut log, vec![Op::set(extra, Done::yes()), Op::Despawn { id: Entity::new() }]), ["op_failed"]);
    assert_eq!(log.graph().save().unwrap(), before);
    assert!(!log.graph().entities().any(|e| e == extra));

    ok(&mut log, vec![choose(c.calib, c.cad)]);
    assert_eq!(refused(&mut log, vec![Op::link(c.cad, c.calib, Alternative { ruled_out: Some(json!({})), weight: None })]), ["chosen_must_be_a_live_option"]);
    assert_eq!(refused(&mut log, vec![Op::set(c.cad, Dropped::yes())]), ["chosen_must_be_a_live_option"], "reaching it from the other end");
}

#[test]
fn gaps_cannot_be_done_chosen_or_given_parts() {
    let mut log = plan_log();
    let (d, gap, task) = (Entity::new(), Entity::new(), Entity::new());
    ok(&mut log, plan::new_decision(d, gap));
    ok(&mut log, vec![Op::set(task, Task {})]);

    assert_eq!(refused(&mut log, vec![Op::set(gap, Done::yes())]), ["unknown_cannot_be_done"]);
    assert_eq!(refused(&mut log, vec![choose(d, gap)]), ["unknown_cannot_be_chosen"]);
    assert_eq!(refused(&mut log, vec![Op::link(task, gap, PartOf)]), ["unknown_has_no_parts"]);

    assert!(log.notices().is_empty(), "the gap counts as an option");
    ok(&mut log, plan::close_gap::<Alternative>(gap, d));
    assert_eq!(log.notices().iter().map(|n| n.rule.as_str()).collect::<Vec<_>>(), ["decision_without_options"]);
    assert_eq!(log.events().last().unwrap().ops.len(), 2);
}

#[test]
fn values_must_fit_what_was_declared_and_what_nothing_declares_is_kept() {
    let mut log = plan_log();
    let (a, b) = (Entity::new(), Entity::new());
    ok(&mut log, vec![Op::Spawn { id: a }, Op::Spawn { id: b }]);

    assert_eq!(refused(&mut log, vec![Op::set_runtime(a, "estimate", json!("soon"))]), ["malformed_component"]);
    assert_eq!(refused(&mut log, vec![Op::set_runtime(a, "part_of", Value::Null)]), ["component_on_wrong_target"]);
    assert_eq!(refused(&mut log, vec![Op::link_runtime(a, b, recollect_graph::Runtime::new("done", Value::Null))]), ["component_on_wrong_target"]);
    assert_eq!(refused(&mut log, vec![Op::link(a, b, Alternative::default())]), ["link_target_lacks_component"], "b isn't a decision");
    assert_eq!(refused(&mut log, vec![Op::link_runtime(a, a, recollect_graph::Runtime::new("note", Value::Null))]), ["no_self_links"]);
    assert_eq!(refused(&mut log, vec![Op::set_rule(a, "x", "not/registered", Value::Null)]), ["rule_cannot_run"], "only a hand edit can leave one of these");

    ok(&mut log, vec![Op::set_runtime(a, "from_a_plugin", json!([1, 2, 3]))]);
    assert_eq!(log.graph().get_runtime(a, "from_a_plugin").unwrap().unwrap().value, json!([1, 2, 3]));
}

#[test]
fn what_if_is_a_commit_that_writes_nothing() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let events = log.events().len();
    let saved = log.graph().save().unwrap();

    let g = log.what_if(&[choose(c.calib, c.cad)]).unwrap();
    assert_eq!(g.get::<Decision>(c.calib), Ok(Some(Decision { chosen: Some(c.cad) })));
    assert_eq!(log.graph().save().unwrap(), saved);
    assert_eq!(log.events().len(), events);

    let refusal = log.what_if(&[choose(c.calib, c.imu)]).err().expect("refused");
    assert_eq!(refusal[0].rule, "chosen_must_be_a_live_option");
    assert_eq!(log.graph().save().unwrap(), saved);

    log.commit("t", "x", vec![choose(c.calib, c.cad)]).unwrap();
    assert_eq!(log.graph().save().unwrap(), g.save().unwrap(), "and the real thing lands in the same place");
}

#[test]
fn a_plugin_declares_its_own_rules_the_same_way() {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    reg.declare("review", Place::Node, None);
    reg.invariant(rule("review_needs_a_reviewer"), |cx: &Cx| {
        let review = cx.get_runtime(cx.id(), "review")?;
        Ok(match review {
            Some(r) if r.value.get("by").is_none() => vec!["nobody is reviewing this".to_string()],
            _ => vec![],
        })
    });
    let mut log = Log::new(Arc::new(reg));
    let a = Entity::new();
    assert_eq!(refused(&mut log, vec![Op::set_runtime(a, "review", json!({}))]), ["review_needs_a_reviewer"]);
    ok(&mut log, vec![Op::set_runtime(a, "review", json!({ "by": "someone" }))]);
    assert_eq!(refused(&mut log, vec![Op::set_runtime(a, "review", json!({}))]), ["review_needs_a_reviewer"], "and it's checked again");
}

#[test]
fn a_check_only_reruns_where_an_edit_reached() {
    let mut log = plan_log();
    let tasks: Vec<Entity> = (0..50).map(|_| Entity::new()).collect();
    ok(&mut log, tasks.iter().map(|t| Op::set(*t, Task {})).collect());
    log.graph().take_ran();

    // No check reached `done` here: the one that could reads `unknown` first, and stops.
    ok(&mut log, vec![Op::set(tasks[7], Done::yes())]);
    assert_eq!(log.graph().take_ran(), vec![]);

    ok(&mut log, vec![Op::set(tasks[7], Decision::default())]);
    let ran = log.graph().take_ran();
    assert!(!ran.is_empty());
    assert!(ran.iter().all(|(e, _)| *e == tasks[7]), "only the node that changed: {ran:?}");
}

// ---------- rules through the log ----------

#[component]
struct Amount(f64);

#[component]
struct Addend;

#[rule("sheet/sum", incoming(Addend))]
struct Sum;

impl Compute for Sum {
    type Output = Amount;
    fn compute(&self, cx: &Cx) -> Result<Amount, Error> {
        let mut total = 0.0;
        for from in cx.sources::<Addend>()? {
            total += cx.value::<Amount>(from)?.0;
        }
        Ok(Amount(total))
    }
}

fn sheet_registry(build: &str) -> Arc<Registry> {
    let mut reg = Registry::new();
    reg.rule::<Sum>().build(build);
    Arc::new(reg)
}

#[test]
fn rules_are_logged_by_key_and_undo_like_anything_else() {
    let mut log = Log::new(sheet_registry("v1"));
    let (price, total) = (Entity::new(), Entity::new());
    ok(&mut log, vec![Op::set(price, Amount(10.0)), Op::set(total, Sum), Op::link(price, total, Addend)]);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(10.0))));

    let typed_over = ok(&mut log, vec![Op::set(total, Amount(99.0))]);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(99.0))));
    log.undo(typed_over, "t", "put the formula back").unwrap();
    assert_eq!(log.graph().get(total), Ok(Some(Amount(10.0))));
    ok(&mut log, vec![Op::set(price, Amount(12.0))]);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(12.0))), "and it's live again");
}

#[test]
#[should_panic(expected = "closure")]
fn a_closure_cant_be_logged() {
    let closure = recollect_graph::rule("adhoc").compute(|_| Ok(Amount(1.0)));
    let _ = Op::set(Entity::new(), closure);
}

// ---------- external edits ----------

#[test]
fn external_edits_are_audited_not_lost() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let mut disk = log.graph().save().unwrap();
    disk.nodes.iter_mut().find(|n| n.id == c.imu).unwrap().components.insert("done".into(), Value::Null);
    disk.nodes.iter_mut().find(|n| n.id == c.free).unwrap().links.clear();
    disk.nodes.retain(|n| n.id != c.free);

    assert_eq!(log.reconcile(&disk, "git pull").unwrap(), vec![]);
    assert_eq!(log.graph().save().unwrap(), disk, "the graph now matches the files");

    let ev = log.blame(Target::Node(c.imu), Done::NAME).unwrap();
    assert_eq!((ev.author.as_str(), ev.why.as_str()), ("git pull", "external edit"));
    let seq = ev.seq;
    assert_eq!(log.state_at(seq).save().unwrap(), disk, "and replay reproduces it");
    assert!(log.state_at(seq - 1).entities().any(|e| e == c.free));

    log.undo(seq, "steven", "that pull was wrong").unwrap();
    assert!(log.graph().entities().any(|e| e == c.free), "external edits can be undone too");
    assert!(!log.graph().has_own(c.imu, Done::NAME), "and it's worked out again");

    let events = log.events().len();
    let same = log.graph().save().unwrap();
    assert_eq!(log.reconcile(&same, "noop").unwrap(), vec![]);
    assert_eq!(log.events().len(), events, "no change, no event");
}

#[test]
fn an_external_edit_that_breaks_a_rule_is_recorded_reported_and_doesnt_block_other_work() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let events = log.events().len();
    let mut disk = log.graph().save().unwrap();
    let node = disk.nodes.iter_mut().find(|n| n.id == c.calib).unwrap();
    node.components.insert("decision".into(), json!({ "chosen": c.imu }));

    let issues = log.reconcile(&disk, "hand edit").unwrap();
    assert_eq!(issues.iter().map(|v| v.rule.as_str()).collect::<Vec<_>>(), ["chosen_must_be_a_live_option"]);
    assert_eq!(log.events().len(), events + 1);

    // Unrelated work can continue, but nothing can newly break the same rule.
    ok(&mut log, vec![Op::set(c.free, Task {})]);
    let other = Entity::new();
    assert!(!refused(&mut log, vec![Op::set(other, Decision { chosen: Some(Entity::new()) })]).is_empty());

    // The fix is an ordinary change.
    ok(&mut log, vec![choose(c.calib, c.cad)]);
    assert_eq!(log.violations(), vec![]);
}

// ---------- on disk ----------

#[test]
fn a_log_survives_being_closed_and_opened() {
    let dir = tempfile::tempdir().unwrap();
    let reg = plan_registry();
    let (saved, c, events) = {
        let (mut log, problems) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
        assert_eq!(problems, vec![]);
        let c = calibration(&mut log);
        let seq = log.commit("steven", "why", vec![choose(c.calib, c.cad)]).unwrap();
        log.undo(seq, "steven", "no").unwrap();
        (log.graph().save().unwrap(), c, log.events().to_vec())
    };
    let (log, problems) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(log.graph().save().unwrap(), saved);
    assert_eq!(log.events(), events, "with its history");
    assert_eq!(log.blame(Target::Node(c.calib), Decision::NAME).unwrap().why, "no");
    drop(log);

    // The files are a plain graph in their own right.
    let (plain, problems) = Graph::load_dir(dir.path(), reg).unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(plain.save().unwrap(), saved);
}

#[test]
fn two_processes_cant_hold_one_log() {
    let dir = tempfile::tempdir().unwrap();
    let first = Log::open(dir.path(), plan_registry(), "ext").unwrap();
    assert!(matches!(Log::open(dir.path(), plan_registry(), "ext"), Err(OpenError::Locked)));
    drop(first);
    Log::open(dir.path(), plan_registry(), "ext").unwrap();
}

#[test]
fn a_hand_edit_to_a_file_is_recorded_when_its_opened() {
    let dir = tempfile::tempdir().unwrap();
    let reg = plan_registry();
    let (mut log, _) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
    let a = Entity::new();
    ok(&mut log, vec![Op::set(a, Task {}), Op::set(a, Estimate::days(1.0))]);
    drop(log);

    let path = dir.path().join("nodes").join(format!("{a}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("estimate: 1.0", "estimate: 5.0")).unwrap();

    let (log, problems) = Log::open(dir.path(), reg.clone(), "me, by hand").unwrap();
    assert_eq!(problems, vec![LoadProblem::ExternalEdit { seq: 1 }]);
    assert_eq!(log.graph().get(a), Ok(Some(Estimate::days(5.0))));
    assert_eq!(log.blame(Target::Node(a), Estimate::NAME).unwrap().author, "me, by hand");
    assert_eq!(log.state_at(0).get(a), Ok(Some(Estimate::days(1.0))), "and what it was before is still known");
    drop(log);

    let (_, problems) = Log::open(dir.path(), reg, "ext").unwrap();
    assert_eq!(problems, vec![], "once");
}

#[test]
fn a_hand_edit_can_break_things_without_losing_them() {
    let dir = tempfile::tempdir().unwrap();
    let reg = plan_registry();
    let (mut log, _) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
    let (a, b) = (Entity::new(), Entity::new());
    ok(&mut log, vec![Op::set(a, Task {}), Op::set(a, Estimate::days(1.0)), Op::set(b, Task {})]);
    drop(log);

    let path = dir.path().join("nodes").join(format!("{a}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("estimate: 1.0", "estimate: soon") + "rules:\n  x:\n    rule: not/registered\n").unwrap();

    let (mut log, problems) = Log::open(dir.path(), reg, "ext").unwrap();
    assert!(problems.contains(&LoadProblem::ExternalEdit { seq: 1 }), "{problems:?}");
    assert!(problems.iter().any(|p| matches!(p, LoadProblem::BrokenRule(_, Error::UnknownRule(k)) if k == "not/registered")), "{problems:?}");
    assert!(problems.iter().any(|p| matches!(p, LoadProblem::Standing(v) if v.iter().any(|v| v.rule == "malformed_component"))), "{problems:?}");

    ok(&mut log, vec![Op::set(b, Done::yes())]);
    assert_eq!(log.graph().get_runtime(a, "estimate").unwrap().unwrap().value, json!("soon"), "it's kept as written");
    assert!(matches!(log.graph().get::<Estimate>(a), Err(Error::Malformed { .. })));
}

#[test]
fn a_new_file_and_a_removed_file_are_edits_too() {
    let dir = tempfile::tempdir().unwrap();
    let reg = plan_registry();
    let (mut log, _) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
    let (a, b) = (Entity::new(), Entity::new());
    ok(&mut log, vec![Op::set(a, Done::yes()), Op::set(b, Done::yes())]);
    drop(log);

    let new = Entity::new();
    std::fs::write(dir.path().join("nodes").join(format!("{new}.yaml")), format!("id: {new}\ncomponents:\n  done: null\nlinks:\n- to: {b}\n  components:\n    part_of: null\n")).unwrap();
    std::fs::remove_file(dir.path().join("nodes").join(format!("{a}.yaml"))).unwrap();

    let (log, problems) = Log::open(dir.path(), reg, "ext").unwrap();
    assert_eq!(problems, vec![LoadProblem::ExternalEdit { seq: 1 }]);
    let ids: Vec<Entity> = log.graph().entities().collect();
    assert!(ids.contains(&new) && ids.contains(&b) && !ids.contains(&a), "{ids:?}");
    log.graph().get::<Done>(new).unwrap().unwrap();
}

#[test]
fn a_directory_of_plain_files_becomes_a_logged_one() {
    let dir = tempfile::tempdir().unwrap();
    let reg = plan_registry();
    let mut g = Graph::with_registry(reg.clone());
    let a = g.spawn();
    g.set(a, Done::yes());
    g.save_dir(dir.path()).unwrap();

    let (log, problems) = Log::open(dir.path(), reg, "first import").unwrap();
    assert_eq!(problems, vec![LoadProblem::ExternalEdit { seq: 0 }]);
    assert_eq!(log.events()[0].author, "first import");
    assert_eq!(log.graph().save().unwrap(), g.save().unwrap());
}

#[test]
fn defaults_are_logged_and_saved_like_any_other_change() {
    let dir = tempfile::tempdir().unwrap();
    let reg = sheet_registry("v1");
    let (mut log, _) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
    let (price, total) = (Entity::new(), Entity::new());
    ok(&mut log, vec![Op::set(price, Amount(10.0)), Op::Spawn { id: total }, Op::link(price, total, Addend)]);

    let d = ok(&mut log, vec![Op::Default { component: "amount".into(), to: Some(recollect_graph::Stored::Rule(recollect_graph::SavedRule { rule: "sheet/sum".into(), params: Value::Null })) }]);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(10.0))));
    drop(log);

    let (mut log, problems) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(10.0))));
    log.undo(d, "t", "no").unwrap();
    assert_eq!(log.graph().get::<Amount>(total), Ok(None));
    drop(log);
    let (log, problems) = Log::open(dir.path(), reg, "ext").unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(log.graph().get::<Amount>(total), Ok(None));
}

// ---------- the audit gap ----------

// The same key, the same signature, and a different answer: what a bug fix looks like.
mod fixed {
    use super::{Addend, Amount, Compute, Cx, Error, rule};

    #[rule("sheet/sum", incoming(Addend))]
    pub(super) struct Sum;

    impl Compute for Sum {
        type Output = Amount;
        fn compute(&self, cx: &Cx) -> Result<Amount, Error> {
            Ok(Amount(super::Sum.compute(cx)?.0 + 1.0))
        }
    }
}

fn fixed_registry(build: &str) -> Arc<Registry> {
    let mut reg = Registry::new();
    reg.rule::<fixed::Sum>().build(build);
    Arc::new(reg)
}

#[test]
fn the_log_says_which_code_each_event_ran_under() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = Log::open(dir.path(), sheet_registry("v1"), "ext").unwrap();
    let a = Entity::new();
    let s0 = ok(&mut log, vec![Op::set(a, Amount(1.0))]);
    drop(log);

    let (mut log, problems) = Log::open(dir.path(), sheet_registry("v2"), "ext").unwrap();
    assert!(matches!(&problems[..], [LoadProblem::CodeChanged { was, now }] if was.build == "v1" && now.build == "v2"), "{problems:?}");
    let s1 = ok(&mut log, vec![Op::set(a, Amount(2.0))]);
    let s2 = ok(&mut log, vec![Op::set(a, Amount(3.0))]);
    drop(log);

    let (log, problems) = Log::open(dir.path(), sheet_registry("v2"), "ext").unwrap();
    assert_eq!(problems, vec![], "recorded, so it isn't news any more");
    assert_eq!(log.code_at(s0).unwrap().build, "v1");
    assert_eq!(log.code_at(s1).unwrap().build, "v2");
    assert_eq!(log.code_at(s2).unwrap().build, "v2");
    assert!(log.code_at(s0).unwrap().rules.iter().any(|r| r.key == "sheet/sum"), "with every rule's signature");
}

#[test]
fn an_audit_catches_a_bug_fix_under_the_same_key() {
    let dir = tempfile::tempdir().unwrap();
    let (mut log, _) = Log::open(dir.path(), sheet_registry("same"), "ext").unwrap();
    let (price, total) = (Entity::new(), Entity::new());
    ok(&mut log, vec![Op::set(price, Amount(10.0)), Op::set(total, Sum), Op::link(price, total, Addend)]);
    let w = log.witness(total, "amount").unwrap();
    assert_eq!(w.seen, Seen::Value(json!(10.0)));
    log.witness(price, "amount").unwrap();
    ok(&mut log, vec![Op::set(price, Amount(20.0))]);
    log.witness(total, "amount").unwrap();
    assert_eq!(log.audit(), vec![], "the same code agrees with itself");
    drop(log);

    // Nothing in the key, signature or build gives this away.
    let (log, problems) = Log::open(dir.path(), fixed_registry("same"), "ext").unwrap();
    assert_eq!(problems, vec![]);
    let drift = log.audit();
    assert_eq!(drift.len(), 2, "both readings of the total, and not the typed-in price: {drift:?}");
    assert_eq!(drift[0].witness.seen, Seen::Value(json!(10.0)));
    assert_eq!(drift[0].now, Seen::Value(json!(11.0)));
    assert_eq!(drift[1].witness.seen, Seen::Value(json!(20.0)));
    assert_eq!(drift[1].now, Seen::Value(json!(21.0)));
    assert_eq!(drift[0].witness.seq, Some(0), "at the point in history it was seen");
}

// ---------- scale ----------

/// A plan with phases of tasks, some waiting on each other: about what a real one looks like.
fn plan_ops(tasks: usize) -> (Vec<Op>, Vec<Entity>) {
    let root = Entity::new();
    let mut ops = vec![Op::set(root, plan::Goal)];
    let mut ids = Vec::new();
    let mut phase = root;
    for i in 0..tasks {
        if i % 20 == 0 {
            let next = Entity::new();
            ops.push(Op::set(next, Name(format!("phase {i}"))));
            ops.push(Op::link(next, root, PartOf));
            if phase != root {
                ops.push(Op::link(next, phase, DependsOn::default()));
            }
            phase = next;
        }
        let t = Entity::new();
        ops.push(Op::set(t, Task {}));
        ops.push(Op::set(t, Estimate::days(1.0)));
        ops.push(Op::link(t, phase, PartOf));
        ids.push(t);
    }
    (ops, ids)
}

#[test]
fn what_a_commit_costs() {
    let sizes: &[usize] = if cfg!(debug_assertions) { &[200] } else { &[1_000, 10_000] };
    for &n in sizes {
        let (ops, ids) = plan_ops(n);
        let edits = 200;

        let mut log = plan_log();
        let start = std::time::Instant::now();
        log.commit("t", "import", ops.clone()).unwrap();
        let import = start.elapsed();
        let start = std::time::Instant::now();
        for i in 0..edits {
            log.commit("t", "edit", vec![Op::set(ids[i * 7 % n], Done::yes())]).unwrap();
        }
        let memory = start.elapsed() / edits as u32;

        let dir = tempfile::tempdir().unwrap();
        let (mut log, _) = Log::open(dir.path(), plan_registry(), "ext").unwrap();
        let start = std::time::Instant::now();
        log.commit("t", "import", ops).unwrap();
        let import_disk = start.elapsed();
        let start = std::time::Instant::now();
        for i in 0..edits {
            log.commit("t", "edit", vec![Op::set(ids[i * 7 % n], Done::yes())]).unwrap();
        }
        let disk = start.elapsed() / edits as u32;
        drop(log);
        let start = std::time::Instant::now();
        let (log, problems) = Log::open(dir.path(), plan_registry(), "ext").unwrap();
        let reopen = start.elapsed();
        assert_eq!(problems, vec![]);
        assert_eq!(log.events().len(), edits + 1);
        println!("{n} tasks: import {import:?} in memory, {import_disk:?} on disk; each edit {memory:?} in memory, {disk:?} on disk; reopen {reopen:?}");
    }
}

#[test]
fn a_very_deep_plan_is_checked_without_running_out_of_stack() {
    let depth = if cfg!(debug_assertions) { 2_000 } else { 20_000 };
    let mut log = plan_log();
    let ids: Vec<Entity> = (0..depth).map(|_| Entity::new()).collect();
    let mut ops: Vec<Op> = ids.iter().map(|e| Op::set(*e, Task {})).collect();
    ops.extend(ids.windows(2).map(|w| Op::link(w[1], w[0], PartOf)));
    let start = std::time::Instant::now();
    log.commit("t", "deep", ops).unwrap();
    println!("{depth} levels: {:?}", start.elapsed());

    // And a loop from the bottom to the top is still found.
    let rules = refused(&mut log, vec![Op::link(ids[0], ids[depth - 1], DependsOn::default())]);
    assert!(rules.iter().all(|r| r == "no_deadlock"), "{:?}", &rules[..1]);
    assert_eq!(log.violations(), vec![]);
}

// ---------- history is a tree ----------

fn note(log: &mut Log, e: Entity, n: f64) -> u64 {
    ok(log, vec![Op::set(e, Estimate::days(n))])
}

fn days(log: &Log, e: Entity) -> Option<Estimate> {
    log.graph().get::<Estimate>(e).unwrap()
}

#[test]
fn a_change_after_going_back_is_a_branch_and_loses_nothing() {
    let mut log = plan_log();
    let t = Entity::new();
    let first = ok(&mut log, vec![Op::set(t, Task {})]);
    let second = note(&mut log, t, 1.0);
    let third = note(&mut log, t, 2.0);

    log.checkout(second, "t", "go back").unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(1.0)));
    assert_eq!(log.last_seq(), Some(second));
    assert_eq!(log.events().len(), 3, "going back adds no event");
    assert_eq!(log.newest_after(second), Some(third));

    let fork = note(&mut log, t, 9.0);
    assert_eq!(log.events()[fork as usize].parent, Some(second), "it follows from where the log is");
    assert_eq!(log.path().iter().map(|e| e.seq).collect::<Vec<_>>(), vec![first, second, fork]);
    assert_eq!(days(&log, t), Some(Estimate::days(9.0)));

    // Both ends are still there, and going between them is exact.
    assert_eq!(log.state_at(third).get(t), Ok(Some(Estimate::days(2.0))));
    assert_eq!(log.state_at(fork).get(t), Ok(Some(Estimate::days(9.0))));
    log.checkout(third, "t", "the other way").unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(2.0)));
    assert_eq!(log.path().iter().map(|e| e.seq).collect::<Vec<_>>(), vec![first, second, third]);

    assert!(matches!(log.checkout(99, "t", ""), Err(recollect_graph::CommitError::NoSuchEvent(99))));
    assert_eq!(log.last_seq(), Some(third), "and that changed nothing");
}

#[test]
fn blame_and_history_only_look_at_what_led_here() {
    let mut log = plan_log();
    let t = Entity::new();
    ok(&mut log, vec![Op::set(t, Task {})]);
    let base = note(&mut log, t, 1.0);
    let other = log.commit("someone else", "on the other branch", vec![Op::set(t, Estimate::days(5.0))]).unwrap();
    log.checkout(base, "t", "").unwrap();
    assert_eq!(log.blame(Target::Node(t), Estimate::NAME).unwrap().seq, base);
    assert_eq!(log.history(Target::Node(t)).len(), 2);
    log.checkout(other, "t", "").unwrap();
    assert_eq!(log.blame(Target::Node(t), Estimate::NAME).unwrap().author, "someone else");
    assert_eq!(log.history(Target::Node(t)).len(), 3);
}

#[test]
fn only_what_led_here_can_be_undone() {
    let mut log = plan_log();
    let t = Entity::new();
    ok(&mut log, vec![Op::set(t, Task {})]);
    let base = note(&mut log, t, 1.0);
    let other = note(&mut log, t, 5.0);
    log.checkout(base, "t", "").unwrap();
    assert!(matches!(log.undo(other, "t", ""), Err(recollect_graph::CommitError::NotOnThisBranch(_))));
    assert_eq!(days(&log, t), Some(Estimate::days(1.0)));
    log.undo(base, "t", "").unwrap();
    assert_eq!(days(&log, t), None, "and undoing is on this branch like anything else");
}

#[test]
fn branches_and_where_it_is_survive_being_closed_and_opened() {
    let dir = tempfile::tempdir().unwrap();
    let reg = plan_registry();
    let t = Entity::new();
    let (base, other, fork);
    {
        let (mut log, _) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
        ok(&mut log, vec![Op::set(t, Task {})]);
        base = note(&mut log, t, 1.0);
        other = note(&mut log, t, 5.0);
        log.checkout(base, "t", "").unwrap();
        fork = note(&mut log, t, 9.0);
        log.checkout(base, "t", "").unwrap();
    }
    let path = dir.path().join("nodes").join(format!("{t}.yaml"));
    assert!(std::fs::read_to_string(&path).unwrap().contains("estimate: 1.0"), "the files show where it is");
    let (mut log, problems) = Log::open(dir.path(), reg.clone(), "ext").unwrap();
    assert_eq!(problems, vec![], "moving around isn't an edit");
    assert_eq!(log.last_seq(), Some(base));
    assert_eq!(days(&log, t), Some(Estimate::days(1.0)));
    log.checkout(other, "t", "").unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(5.0)));
    log.checkout(fork, "t", "").unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(9.0)));
    assert!(std::fs::read_to_string(&path).unwrap().contains("estimate: 9.0"));

    // A hand edit made here is on this branch.
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("estimate: 9.0", "estimate: 3.0")).unwrap();
    assert!(log.reload("me").unwrap().is_some());
    let edit = log.last_seq().unwrap();
    assert_eq!(log.events()[edit as usize].parent, Some(fork));
    drop(log);
    let (log, problems) = Log::open(dir.path(), reg, "ext").unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(log.state_at(other).get(t), Ok(Some(Estimate::days(5.0))), "and the other branch is untouched");
}

/// Moving around at random has to land exactly where replaying from nothing would, and leave no stale
/// values behind in what's remembered.
#[test]
fn going_anywhere_is_the_same_as_replaying_from_nothing() {
    let mut log = plan_log();
    let tasks: Vec<Entity> = (0..6).map(|_| Entity::new()).collect();
    let (d, a, b) = (Entity::new(), Entity::new(), Entity::new());
    ok(&mut log, vec![Op::set(d, Decision::default()), Op::Spawn { id: a }, Op::Spawn { id: b }, Op::link(a, d, Alternative::default()), Op::link(b, d, Alternative::default())]);
    let mut state = 0x9E3779B97F4A7C15u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let (mut moves, mut forks) = (0, 0);
    for _ in 0..400 {
        let t = tasks[next(tasks.len())];
        match next(6) {
            0 | 1 => {
                let seq = next(log.events().len()) as u64;
                log.checkout(seq, "t", "").unwrap();
                moves += 1;
            }
            2 => {
                forks += usize::from(log.last_seq().is_some_and(|h| log.newest_after(h).is_some()));
                let _ = log.commit("t", "", vec![Op::set(t, Task {}), Op::link(t, if next(2) == 0 { a } else { b }, PartOf)]);
            }
            3 => {
                let _ = log.commit("t", "", vec![Op::set(t, Estimate::days(next(9) as f64))]);
            }
            4 => {
                let _ = log.commit("t", "", vec![Op::set(d, Decision { chosen: Some(if next(2) == 0 { a } else { b }) })]);
            }
            _ => {
                let _ = log.commit("t", "", vec![Op::set(d, Decision::default())]);
            }
        }
        let head = log.last_seq().unwrap();
        let fresh = log.state_at(head);
        assert_eq!(log.graph().save().unwrap(), fresh.save().unwrap(), "at {head}");
        for e in fresh.entities() {
            for c in fresh.components_of(e) {
                assert_eq!(log.graph().get_runtime(e, &c), fresh.get_runtime(e, &c), "{c} at {head}");
            }
        }
    }
    assert!(moves > 50 && forks > 10, "{moves} moves and {forks} forks: the test should really exercise this");
}

#[test]
fn what_to_return_to_is_only_ever_on_the_way_from_here() {
    let mut log = plan_log();
    let t = Entity::new();
    let start = ok(&mut log, vec![Op::set(t, Task {})]);
    let one = note(&mut log, t, 1.0);
    log.checkout(start, "t", "").unwrap();
    let other = note(&mut log, t, 2.0);
    assert_eq!(log.newest_after(one), None, "the newer event is on a branch this doesn't lead to");
    assert_eq!(log.newest_after(start), Some(other));
    assert_eq!(log.newest_after(other), None);
}
