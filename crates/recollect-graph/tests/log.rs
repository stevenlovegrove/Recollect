//! The log and invariants: who changed what and why, what gets refused, and what happens when the
//! files change underneath it.

use std::sync::Arc;

use recollect_graph::{Cell, CellType, Cx, Entity, Error, Graph, LoadProblem, Log, Name, On, Op, Registry, SavedGraph, Severity, Stored, Target, cell};
use recollect_systems::formulas::LABEL;
use recollect_systems::formulas::Chosen;
use recollect_systems::plan::{self, ALTERNATIVE, DECISION, DEPENDS_ON, Done, Dropped, Estimate, GOAL, PART_OF, RuledOut, TASK};
use serde_json::json;

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

/// A new node with a component.
fn new(id: Entity, component: &str) -> [Op; 2] {
    [Op::Spawn { id }, Op::add(id, component)]
}

/// A new task.
fn task(id: Entity) -> [Op; 2] {
    new(id, TASK)
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
    let mut ops = new(calib, DECISION).to_vec();
    ops.extend([Op::Spawn { id: cad }, Op::set(cad, Name("cad".into())), Op::Spawn { id: free }, Op::set(free, Name("free".into())), Op::link(cad, calib, ALTERNATIVE), Op::link(free, calib, ALTERNATIVE)]);
    ok(log, ops);
    let mut ops = task(imu).to_vec();
    ops.extend([Op::set(imu, Estimate::days(1.0)), Op::link(imu, cad, PART_OF)]);
    ok(log, ops);
    Calib { calib, cad, free, imu }
}

fn choose(decision: Entity, option: Entity) -> Op {
    plan::choose(decision, option)
}

fn node(id: Entity) -> On {
    On::Node { id }
}

// ---------- the log answers who, when and why ----------

#[test]
fn the_log_answers_who_when_why() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let seq = log.commit("steven", "CAD model is already accurate", vec![choose(c.calib, c.cad)]).unwrap();

    let ev = log.blame(node(c.calib), Chosen::NAME).unwrap();
    assert_eq!((ev.seq, ev.author.as_str(), ev.why.as_str()), (seq, "steven", "CAD model is already accurate"));
    assert!(ev.at > 0, "and when");

    // Replay reproduces the graph exactly, at any point.
    assert_eq!(log.state_at(seq).save(), log.graph().save());
    assert_eq!(log.state_at(seq - 1).stored(c.calib, Chosen::NAME), None);

    // Undo appends: history isn't rewritten.
    let u = log.undo(seq, "steven", "changed my mind").unwrap();
    assert_eq!(log.graph().stored(c.calib, Chosen::NAME), None);
    assert_eq!(log.state_at(u - 1).get::<Chosen>(c.calib), Ok(Some(Chosen::on(c.cad))));
    assert_eq!(log.history(Target::Node(c.calib)).iter().filter(|e| e.seq >= seq).count(), 2);
    assert!(log.blame(node(c.free), "nothing").is_none());
}

#[test]
fn undoing_a_creation_removes_it_rather_than_leaving_it_empty() {
    let mut log = plan_log();
    let (a, b) = (Entity::new(), Entity::new());
    let mut ops = task(a).to_vec();
    ops.extend([Op::Spawn { id: b }, Op::link(a, b, PART_OF)]);
    let s = ok(&mut log, ops);
    log.undo(s, "t", "undo").unwrap();
    assert_eq!(log.graph().save(), SavedGraph::default());
}

#[test]
fn every_kind_of_op_survives_being_written_down() {
    let (a, b) = (Entity::new(), Entity::new());
    let ops = vec![
        Op::Spawn { id: a },
        Op::Despawn { id: a },
        Op::add(a, TASK),
        Op::Remove { id: a, component: TASK.into() },
        Op::set(a, Done::yes()),
        Op::set_value(a, "note", json!({ "text": null })),
        // A null value isn't the same as no value.
        Op::set_value(a, "note", json!(null)),
        Op::set_formula(a, "x", "1 + 1"),
        Op::Set { on: node(a), cell: "x".into(), to: None },
        Op::link(a, b, PART_OF),
        Op::Set { on: On::Edge { from: a, to: b }, cell: "note".into(), to: Some(Stored::Value(json!(1))) },
        Op::Set { on: On::Edge { from: a, to: b }, cell: "note".into(), to: Some(Stored::Formula("2".into())) },
        Op::Unlink { from: a, to: b, component: PART_OF.into() },
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
    let mut ops = task(parent).to_vec();
    ops.extend(task(child));
    ops.push(Op::link(child, parent, PART_OF));
    ok(&mut log, ops);
    let rules = refused(&mut log, vec![Op::link(child, parent, DEPENDS_ON)]);
    assert!(rules.iter().all(|r| r == "no_deadlock"), "{rules:?}");

    // Two phases whose steps wait on each other across the hierarchy.
    let mut log = plan_log();
    let (a, b, a1, b1) = (Entity::new(), Entity::new(), Entity::new(), Entity::new());
    ok(&mut log, vec![Op::Spawn { id: a }, Op::Spawn { id: b }, Op::Spawn { id: a1 }, Op::Spawn { id: b1 }, Op::link(a1, a, PART_OF), Op::link(b1, b, PART_OF)]);
    ok(&mut log, vec![Op::link(a, b, DEPENDS_ON)]);
    let rules = refused(&mut log, vec![Op::link(b1, a1, DEPENDS_ON)]);
    assert!(rules.contains(&"no_deadlock".to_string()), "a1 waits for b, which needs b1, which waits for a1: {rules:?}");
    assert!(log.violations().is_empty(), "and it's as if it never happened");
}

#[test]
fn a_refused_commit_changes_nothing_and_leaves_no_event() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let before = log.graph().save();
    let events = log.events().len();

    assert_eq!(refused(&mut log, vec![choose(c.calib, c.imu)]), ["chosen_must_be_a_live_option"], "imu isn't one of its options");
    assert_eq!(refused(&mut log, vec![choose(c.calib, Entity::new())]), ["chosen_must_be_a_live_option"]);
    assert_eq!(log.graph().save(), before);
    assert_eq!(log.events().len(), events);

    // A change that's partly fine is refused as a whole, including what came before the bad part.
    let extra = Entity::new();
    let mut fine = task(extra).to_vec();
    fine.push(Op::set(extra, Done::yes()));
    assert_eq!(refused(&mut log, [fine.clone(), vec![choose(c.calib, c.imu)]].concat()), ["chosen_must_be_a_live_option"]);
    assert_eq!(refused(&mut log, [fine, vec![Op::Despawn { id: Entity::new() }]].concat()), ["op_failed"]);
    assert_eq!(log.graph().save(), before);
    assert!(!log.graph().entities().any(|e| e == extra));

    ok(&mut log, vec![choose(c.calib, c.cad)]);
    let rule_out = Op::Set { on: On::Edge { from: c.cad, to: c.calib }, cell: RuledOut::NAME.into(), to: Some(Stored::Value(json!({}))) };
    assert_eq!(refused(&mut log, vec![rule_out]), ["chosen_must_be_a_live_option"]);
    assert_eq!(refused(&mut log, vec![Op::set(c.cad, Dropped::yes())]), ["chosen_must_be_a_live_option"], "reaching it from the other end");
}

#[test]
fn gaps_cannot_be_done_chosen_or_given_parts() {
    let mut log = plan_log();
    let (d, gap, t) = (Entity::new(), Entity::new(), Entity::new());
    let mut ops = vec![Op::Spawn { id: d }, Op::Spawn { id: gap }];
    ops.extend(plan::new_decision(d, gap));
    ok(&mut log, ops);
    ok(&mut log, task(t).to_vec());

    assert_eq!(refused(&mut log, vec![Op::set(gap, Done::yes())]), ["unknown_cannot_be_done"]);
    assert_eq!(refused(&mut log, vec![choose(d, gap)]), ["unknown_cannot_be_chosen"]);
    assert_eq!(refused(&mut log, vec![Op::link(t, gap, PART_OF)]), ["unknown_has_no_parts"]);

    assert!(log.notices().is_empty(), "the gap counts as an option");
    ok(&mut log, plan::close_gap(gap, d, ALTERNATIVE));
    assert_eq!(log.notices().iter().map(|n| n.rule.as_str()).collect::<Vec<_>>(), ["decision_without_options"]);
    assert_eq!(log.events().last().unwrap().ops.len(), 2);
}

#[test]
fn values_must_fit_what_was_declared_and_what_nothing_declares_is_kept() {
    let mut log = plan_log();
    let (a, b) = (Entity::new(), Entity::new());
    let mut ops = task(a).to_vec();
    ops.push(Op::Spawn { id: b });
    ok(&mut log, ops);

    assert_eq!(refused(&mut log, vec![Op::set_value(a, "estimate", json!("soon"))]), ["malformed_cell"]);
    assert_eq!(refused(&mut log, vec![Op::set(b, Estimate::days(1.0))]), ["cell_without_component"], "b isn't a task");
    assert_eq!(refused(&mut log, vec![Op::add(a, "nonsense")]), ["unknown_component"]);
    assert_eq!(refused(&mut log, vec![Op::add(a, DEPENDS_ON)]), ["component_on_wrong_target"]);
    assert_eq!(refused(&mut log, vec![Op::link(a, b, TASK)]), ["component_on_wrong_target"]);
    assert_eq!(refused(&mut log, vec![Op::link(a, a, LABEL)]), ["no_self_links"]);
    assert_eq!(refused(&mut log, vec![Op::set_formula(a, "x", "1 +")]), ["formula_cannot_be_read"], "only a hand edit can leave one of these");

    ok(&mut log, vec![Op::set_value(a, "from_a_plugin", json!([1, 2, 3]))]);
    assert_eq!(log.graph().get_value(a, "from_a_plugin").unwrap(), Some(json!([1, 2, 3])));

    // An edge brings what it needs to its ends, so linking an option to something makes that a decision.
    ok(&mut log, vec![Op::link(a, b, ALTERNATIVE)]);
    assert!(log.graph().has(b, DECISION), "b is a decision now");
}

/// The same as giving it `depends_on`, above, but `part_of` has a check of its own, which expects an edge.
#[test]
fn an_edge_component_with_a_check_on_a_node_is_only_on_the_wrong_target() {
    let mut log = plan_log();
    let a = Entity::new();
    ok(&mut log, task(a).to_vec());
    assert_eq!(refused(&mut log, vec![Op::add(a, PART_OF)]), ["component_on_wrong_target"]);
}

#[test]
fn what_if_is_a_commit_that_writes_nothing() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let events = log.events().len();
    let saved = log.graph().save();

    let g = log.what_if(&[choose(c.calib, c.cad)]).unwrap();
    assert_eq!(g.get::<Chosen>(c.calib), Ok(Some(Chosen::on(c.cad))));
    assert_eq!(log.graph().save(), saved);
    assert_eq!(log.events().len(), events);

    let refusal = log.what_if(&[choose(c.calib, c.imu)]).err().expect("refused");
    assert_eq!(refusal[0].rule, "chosen_must_be_a_live_option");
    assert_eq!(log.graph().save(), saved);

    log.commit("t", "x", vec![choose(c.calib, c.cad)]).unwrap();
    assert_eq!(log.graph().save(), g.save(), "and the real thing lands in the same place");
}

#[cell]
struct Reviewer(String);

#[test]
fn a_plugin_declares_its_own_checks_the_same_way() {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    reg.component("review", "Being reviewed.");
    reg.cell::<Reviewer>("review", "Who's reviewing it.");
    reg.check("review", "review_needs_a_reviewer", Severity::Invariant, |cx: &Cx| Ok(if cx.own::<Reviewer>()?.is_none() { vec!["nobody is reviewing this".to_string()] } else { vec![] }));
    let mut log = Log::new(Arc::new(reg));
    let a = Entity::new();
    assert_eq!(refused(&mut log, new(a, "review").to_vec()), ["review_needs_a_reviewer"]);
    let mut ops = new(a, "review").to_vec();
    ops.push(Op::set(a, Reviewer("someone".into())));
    ok(&mut log, ops);
    assert_eq!(refused(&mut log, vec![Op::Set { on: node(a), cell: Reviewer::NAME.into(), to: None }]), ["review_needs_a_reviewer"], "and it's checked again");
}

#[test]
fn a_check_only_reruns_where_an_edit_reached() {
    let mut log = plan_log();
    let tasks: Vec<Entity> = (0..50).map(|_| Entity::new()).collect();
    ok(&mut log, tasks.iter().flat_map(|t| task(*t)).collect());
    log.graph().take_ran();

    // No check reached `done` here: the one that could is only on gaps.
    ok(&mut log, vec![Op::set(tasks[7], Done::yes())]);
    assert_eq!(log.graph().take_ran(), vec![]);

    ok(&mut log, vec![Op::add(tasks[7], DECISION)]);
    let ran = log.graph().take_ran();
    assert!(!ran.is_empty());
    assert!(ran.iter().all(|(e, _)| *e == tasks[7]), "only the node that changed: {ran:?}");
}

// ---------- computed cells through the log ----------

#[cell]
struct Amount(f64);

const SHEET: &str = "sheet";
const ADDEND: &str = "addend";

fn sum(cx: &Cx) -> Cell<Amount> {
    let mut total = 0.0;
    for from in cx.sources(ADDEND) {
        total += cx.value::<Amount>(from)?.0;
    }
    Ok(Some(Amount(total)))
}

fn sheet_registry(build: &str) -> Arc<Registry> {
    let mut reg = Registry::new();
    reg.component(SHEET, "A cell of a sheet: what's added to it, unless a value is typed in.");
    reg.cell::<Amount>(SHEET, "The sum of what's added to it.").builtin(sum);
    reg.edge(ADDEND, "Added to that.");
    reg.build(build);
    Arc::new(reg)
}

#[test]
fn a_value_typed_over_a_builtin_undoes_like_anything_else() {
    let mut log = Log::new(sheet_registry("v1"));
    let (price, total) = (Entity::new(), Entity::new());
    let mut ops = new(price, SHEET).to_vec();
    ops.push(Op::set(price, Amount(10.0)));
    ops.extend(new(total, SHEET));
    ops.push(Op::link(price, total, ADDEND));
    ok(&mut log, ops);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(10.0))));

    let typed_over = ok(&mut log, vec![Op::set(total, Amount(99.0))]);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(99.0))));
    log.undo(typed_over, "t", "put the sum back").unwrap();
    assert_eq!(log.graph().get(total), Ok(Some(Amount(10.0))));
    ok(&mut log, vec![Op::set(price, Amount(12.0))]);
    assert_eq!(log.graph().get(total), Ok(Some(Amount(12.0))), "and it's live again");
}

// ---------- external edits ----------

#[test]
fn external_edits_are_audited_not_lost() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let mut disk = log.graph().save();
    disk.nodes.iter_mut().find(|n| n.id == c.imu).unwrap().cells.insert(Done::NAME.into(), serde_json::to_value(Done::yes()).unwrap());
    disk.nodes.iter_mut().find(|n| n.id == c.free).unwrap().links.clear();
    disk.nodes.retain(|n| n.id != c.free);

    assert_eq!(log.reconcile(&disk, "git pull").1, vec![]);
    assert_eq!(log.graph().save(), disk, "the graph now matches the files");

    let ev = log.blame(node(c.imu), Done::NAME).unwrap();
    assert_eq!((ev.author.as_str(), ev.why.as_str()), ("git pull", "external edit"));
    let seq = ev.seq;
    assert_eq!(log.state_at(seq).save(), disk, "and replay reproduces it");
    assert!(log.state_at(seq - 1).entities().any(|e| e == c.free));

    log.undo(seq, "steven", "that pull was wrong").unwrap();
    assert!(log.graph().entities().any(|e| e == c.free), "external edits can be undone too");
    assert!(!log.graph().has_own(c.imu, Done::NAME), "and it's worked out again");

    let events = log.events().len();
    let same = log.graph().save();
    assert_eq!(log.reconcile(&same, "noop"), (None, vec![]));
    assert_eq!(log.events().len(), events, "no change, no event");
}

#[test]
fn an_external_edit_that_breaks_a_check_is_recorded_reported_and_doesnt_block_other_work() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let events = log.events().len();
    let mut disk = log.graph().save();
    let calib = disk.nodes.iter_mut().find(|n| n.id == c.calib).unwrap();
    calib.cells.insert(Chosen::NAME.into(), json!({ "ref": c.imu }));

    let (_, issues) = log.reconcile(&disk, "hand edit");
    assert_eq!(issues.iter().map(|v| v.rule.as_str()).collect::<Vec<_>>(), ["chosen_must_be_a_live_option"]);
    assert_eq!(log.events().len(), events + 1);

    // Unrelated work can continue, but nothing can newly break the same check.
    ok(&mut log, vec![Op::add(c.free, TASK)]);
    let other = Entity::new();
    let mut ops = new(other, DECISION).to_vec();
    ops.push(choose(other, Entity::new()));
    assert!(!refused(&mut log, ops).is_empty());

    // The fix is an ordinary change.
    ok(&mut log, vec![choose(c.calib, c.cad)]);
    assert_eq!(log.violations(), vec![]);
}

// ---------- loading and what to write ----------

#[test]
fn a_saved_graph_opens_with_no_history_and_what_it_was_is_known() {
    let reg = plan_registry();
    let mut first = Log::new(reg.clone());
    let c = calibration(&mut first);
    let saved = first.graph().save();

    let (mut log, problems) = Log::from_saved(saved.clone(), reg);
    assert_eq!(problems, vec![]);
    assert_eq!(log.graph().save(), saved);
    assert_eq!(log.events().len(), 0, "history is only for while it's open");
    assert_eq!(log.take_changes(), recollect_graph::Changes::default(), "and nothing needs writing");

    let seq = log.commit("steven", "why", vec![choose(c.calib, c.cad)]).unwrap();
    assert_eq!(log.state_at(seq).save(), log.graph().save());
    assert_eq!(log.undo_last("steven").unwrap(), Some(seq + 1));
    assert_eq!(log.graph().save(), saved, "undo goes back to where it was opened");
    assert_eq!(log.undo_last("steven").unwrap(), None, "and no further");
}

#[test]
fn only_the_files_a_change_touched_need_writing() {
    let mut log = plan_log();
    let c = calibration(&mut log);
    let all = log.take_changes();
    assert_eq!(all.nodes.len(), 4);
    assert!(log.take_changes().is_empty(), "once");

    ok(&mut log, vec![choose(c.calib, c.cad)]);
    let changes = log.take_changes();
    assert_eq!(changes.nodes.iter().map(|n| n.id).collect::<Vec<_>>(), vec![c.calib]);

    // An edge lives in its source's file.
    ok(&mut log, vec![Op::Set { on: On::Edge { from: c.imu, to: c.cad }, cell: "note".into(), to: Some(Stored::Value(json!(1))) }]);
    assert_eq!(log.take_changes().nodes.iter().map(|n| n.id).collect::<Vec<_>>(), vec![c.imu]);

    ok(&mut log, vec![Op::Unlink { from: c.imu, to: c.cad, component: PART_OF.into() }, Op::Despawn { id: c.imu }]);
    let changes = log.take_changes();
    assert_eq!((changes.nodes, changes.gone), (vec![], vec![c.imu]));
}

#[test]
fn a_hand_edit_can_break_things_without_losing_them() {
    let reg = plan_registry();
    let (a, b) = (Entity::new(), Entity::new());
    let text = format!("id: {a}\ncomponents:\n- task\ncells:\n  estimate: soon\nformulas:\n  x: 1 +\n");
    let mut saved = SavedGraph::from_files([(format!("nodes/{a}.yaml").as_str(), text.as_str())]).unwrap();
    saved.nodes.push(recollect_graph::SavedNode { components: vec![TASK.into()], ..recollect_graph::SavedNode::empty(b) });

    let (mut log, problems) = Log::from_saved(saved, reg);
    assert!(problems.iter().any(|p| matches!(p, LoadProblem::BrokenFormula((e, c), Error::Failed(_)) if *e == a && c == "x")), "{problems:?}");
    assert!(problems.iter().any(|p| matches!(p, LoadProblem::Standing(v) if v.iter().any(|v| v.rule == "malformed_cell"))), "{problems:?}");

    ok(&mut log, vec![Op::set(b, Done::yes())]);
    assert_eq!(log.graph().get_value(a, "estimate").unwrap(), Some(json!("soon")), "it's kept as written");
    assert!(matches!(log.graph().get::<Estimate>(a), Err(Error::Malformed { .. })));
}

#[test]
fn a_new_file_and_a_removed_file_are_edits_too() {
    let mut log = plan_log();
    let (a, b) = (Entity::new(), Entity::new());
    let mut ops = task(a).to_vec();
    ops.push(Op::set(a, Done::yes()));
    ops.extend(task(b));
    ops.push(Op::set(b, Done::yes()));
    ok(&mut log, ops);
    log.take_changes();

    let new = Entity::new();
    let mut files = log.graph().save().to_files();
    files.insert(format!("nodes/{new}.yaml"), format!("id: {new}\ncomponents:\n- task\ncells:\n  done: true\nlinks:\n- to: {b}\n  components:\n  - part_of\n"));
    files.remove(&format!("nodes/{a}.yaml"));
    let disk = SavedGraph::from_files(files.iter().map(|(p, t)| (p.as_str(), t.as_str()))).unwrap();

    let (seq, _) = log.reconcile(&disk, "ext");
    assert_eq!(log.events()[seq.unwrap() as usize].author, "ext");
    let ids: Vec<Entity> = log.graph().entities().collect();
    assert!(ids.contains(&new) && ids.contains(&b) && !ids.contains(&a), "{ids:?}");
    log.graph().get::<Done>(new).unwrap().unwrap();
    let changes = log.take_changes();
    assert!(changes.gone.contains(&a), "{changes:?}");
}

#[test]
fn an_edge_to_a_node_with_no_file_gets_an_empty_node_and_a_file() {
    let reg = plan_registry();
    let (a, gone) = (Entity::new(), Entity::new());
    let text = format!("id: {a}\nlinks:\n- to: {gone}\n  components:\n  - part_of\n");
    let saved = SavedGraph::from_files([(format!("nodes/{a}.yaml").as_str(), text.as_str())]).unwrap();
    let (mut log, problems) = Log::from_saved(saved, reg);
    assert!(problems.contains(&LoadProblem::DanglingLink { from: a, to: gone }), "{problems:?}");
    assert!(log.graph().contains(gone));
    assert_eq!(log.take_changes().nodes.iter().map(|n| n.id).collect::<Vec<_>>(), vec![gone]);

    // The same from a hand edit.
    let other = Entity::new();
    let mut disk = log.graph().save();
    disk.nodes.iter_mut().find(|n| n.id == a).unwrap().links.push(recollect_graph::SavedEdge { to: other, components: vec![PART_OF.into()], cells: Default::default(), formulas: Default::default() });
    log.reconcile(&disk, "ext");
    assert!(log.graph().contains(other));
    assert!(log.take_changes().nodes.iter().any(|n| n.id == other));
}

#[test]
fn files_are_a_plain_graph_in_their_own_right() {
    let dir = tempfile::tempdir().unwrap();
    let reg = plan_registry();
    let mut log = plan_log();
    calibration(&mut log);
    log.graph().save_dir(dir.path()).unwrap();
    let (plain, problems) = Graph::load_dir(dir.path(), reg).unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(plain.save(), log.graph().save());
}

// ---------- scale ----------

/// A plan with phases of tasks, some waiting on each other: about what a real one looks like.
fn plan_ops(tasks: usize) -> (Vec<Op>, Vec<Entity>) {
    let root = Entity::new();
    let mut ops = new(root, GOAL).to_vec();
    let mut ids = Vec::new();
    let mut phase = root;
    for i in 0..tasks {
        if i % 20 == 0 {
            let next = Entity::new();
            ops.push(Op::Spawn { id: next });
            ops.push(Op::set(next, Name(format!("phase {i}"))));
            ops.push(Op::link(next, root, PART_OF));
            if phase != root {
                ops.push(Op::link(next, phase, DEPENDS_ON));
            }
            phase = next;
        }
        let t = Entity::new();
        ops.extend(task(t));
        ops.push(Op::set(t, Estimate::days(1.0)));
        ops.push(Op::link(t, phase, PART_OF));
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

        println!("{n} tasks: import {import:?}; each edit {memory:?}");
    }
}

#[test]
fn a_very_deep_plan_is_checked_without_running_out_of_stack() {
    let depth = if cfg!(debug_assertions) { 2_000 } else { 20_000 };
    let mut log = plan_log();
    let ids: Vec<Entity> = (0..depth).map(|_| Entity::new()).collect();
    let mut ops: Vec<Op> = ids.iter().flat_map(|e| task(*e)).collect();
    ops.extend(ids.windows(2).map(|w| Op::link(w[1], w[0], PART_OF)));
    let start = std::time::Instant::now();
    log.commit("t", "deep", ops).unwrap();
    println!("{depth} levels: {:?}", start.elapsed());

    // And a loop from the bottom to the top is still found.
    let rules = refused(&mut log, vec![Op::link(ids[0], ids[depth - 1], DEPENDS_ON)]);
    assert!(rules.iter().all(|r| r == "no_deadlock"), "{:?}", &rules[..1]);
    assert_eq!(log.violations(), vec![]);
}

// ---------- undo and redo ----------

fn note(log: &mut Log, e: Entity, n: f64) -> u64 {
    ok(log, vec![Op::set(e, Estimate::days(n))])
}

fn days(log: &Log, e: Entity) -> Option<Estimate> {
    log.graph().get::<Estimate>(e).unwrap()
}

#[test]
fn undo_and_redo_walk_back_and_forward_and_a_change_ends_redo() {
    let mut log = plan_log();
    let t = Entity::new();
    ok(&mut log, task(t).to_vec());
    let one = note(&mut log, t, 1.0);
    let two = note(&mut log, t, 2.0);

    let u = log.undo_last("t").unwrap().unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(1.0)));
    assert_eq!(log.events()[u as usize].reverts, Some(two));
    log.undo_last("t").unwrap();
    assert_eq!(days(&log, t), None);
    assert_eq!(log.redo_next(), Some(u + 1));

    let r = log.redo("t").unwrap().unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(1.0)));
    assert_eq!(log.events()[r as usize].why, format!("redo #{one}"));
    log.redo("t").unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(2.0)));
    assert_eq!(log.redo("t").unwrap(), None, "nothing left to redo");

    // A redo can itself be undone.
    log.undo_last("t").unwrap();
    assert_eq!(days(&log, t), Some(Estimate::days(1.0)));
    note(&mut log, t, 7.0);
    assert_eq!(log.redo_next(), None, "a new change ends what could be redone");
    assert_eq!(log.events().len(), 9, "and every step is an event");
}

#[test]
fn an_undo_that_no_longer_applies_is_refused_and_changes_nothing() {
    let mut log = plan_log();
    let (d, a, b) = (Entity::new(), Entity::new(), Entity::new());
    let mut ops = new(d, DECISION).to_vec();
    ops.extend([Op::Spawn { id: a }, Op::Spawn { id: b }, Op::link(a, d, ALTERNATIVE), Op::link(b, d, ALTERNATIVE)]);
    ok(&mut log, ops);
    let ruled = ok(&mut log, vec![Op::Set { on: On::Edge { from: a, to: d }, cell: RuledOut::NAME.into(), to: Some(Stored::Value(json!("too slow"))) }]);
    ok(&mut log, vec![choose(d, b)]);
    let unruled = log.undo(ruled, "t", "").unwrap();
    ok(&mut log, vec![choose(d, a)]);
    let events = log.events().len();
    let before = log.graph().save();
    // Taking back the undo would rule `a` out again, but `a` is chosen now.
    let err = log.undo(unruled, "t", "").expect_err("refused");
    assert_eq!(err.violations()[0].rule, "chosen_must_be_a_live_option");
    assert_eq!((log.graph().save(), log.events().len()), (before, events));
}

/// Undoing and redoing at random has to land exactly where replaying from nothing would, and leave no
/// stale values behind in what's remembered.
#[test]
fn undoing_and_redoing_anywhere_is_the_same_as_replaying_from_nothing() {
    let mut log = plan_log();
    let tasks: Vec<Entity> = (0..6).map(|_| Entity::new()).collect();
    let (d, a, b) = (Entity::new(), Entity::new(), Entity::new());
    let mut ops = new(d, DECISION).to_vec();
    ops.extend([Op::Spawn { id: a }, Op::Spawn { id: b }, Op::link(a, d, ALTERNATIVE), Op::link(b, d, ALTERNATIVE)]);
    ops.extend(tasks.iter().map(|t| Op::Spawn { id: *t }));
    ok(&mut log, ops);
    let mut state = 0x9E3779B97F4A7C15u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let (mut undos, mut redos) = (0, 0);
    for _ in 0..400 {
        let t = tasks[next(tasks.len())];
        match next(7) {
            0 | 1 => undos += usize::from(log.undo_last("t").is_ok_and(|s| s.is_some())),
            2 => redos += usize::from(log.redo("t").is_ok_and(|s| s.is_some())),
            3 => {
                let _ = log.commit("t", "", vec![Op::add(t, TASK), Op::link(t, if next(2) == 0 { a } else { b }, PART_OF)]);
            }
            4 => {
                let _ = log.commit("t", "", vec![Op::set(t, Estimate::days(next(9) as f64))]);
            }
            5 => {
                let _ = log.commit("t", "", vec![choose(d, if next(2) == 0 { a } else { b })]);
            }
            _ => {
                let _ = log.commit("t", "", vec![plan::reopen(d)]);
            }
        }
        let head = log.last_seq().unwrap();
        let fresh = log.state_at(head);
        assert_eq!(log.graph().save(), fresh.save(), "at {head}");
        for e in fresh.entities().chain(fresh.edges().map(|(e, _, _)| e)) {
            assert_eq!(log.graph().components_of(e), fresh.components_of(e), "at {head}");
            assert_eq!(log.graph().cells_of(e), fresh.cells_of(e), "at {head}");
            for c in fresh.cells_of(e) {
                assert_eq!(log.graph().get_value(e, &c), fresh.get_value(e, &c), "{c} at {head}");
            }
        }
    }
    assert!(undos > 50 && redos > 10, "{undos} undos and {redos} redos: the test should really exercise this");
}
