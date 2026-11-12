//! Long chains of cells, worked out in stretches as the web has to, give the same answers as worked
//! out in one go. The limit is for the whole process, so these are a test binary of their own.

use std::sync::Arc;

use recollect_graph::{Entity, Error, Graph, Log, Op, Registry, set_depth_limit};
use recollect_systems::plan::{self, PART_OF, TASK};
use recollect_systems::samples::{SAMPLES, ops_of};
use serde_json::{Value, json};

fn log_of(systems: &[&str], ops: Vec<Op>) -> Log {
    let names: Vec<String> = systems.iter().map(|s| s.to_string()).collect();
    let mut log = Log::new(recollect_systems::build(&names).unwrap().registry);
    log.commit("t", "", ops).unwrap();
    log
}

/// Every cell of every entity, as JSON or the error it gives.
fn everything(g: &Graph) -> Vec<(Entity, String, Result<Option<Value>, Error>)> {
    let mut out = Vec::new();
    for e in g.entities().chain(g.edges().map(|(e, _, _)| e)) {
        for c in g.cells_of(e) {
            out.push((e, c.clone(), g.get_value(e, &c)));
        }
    }
    out
}

/// A chain of formulas, each one more than the one before.
fn chain(n: usize) -> (Vec<Op>, Vec<Entity>) {
    let ids: Vec<Entity> = (0..n).map(|_| Entity::new()).collect();
    let mut ops = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        ops.push(Op::Spawn { id: *id });
        ops.push(Op::add(*id, "value"));
        if i == 0 {
            ops.push(Op::set_value(*id, "value", json!(1.0)));
        } else {
            ops.push(Op::link(*id, ids[i - 1], "label"));
            ops.push(Op::Set { on: recollect_graph::On::Edge { from: *id, to: ids[i - 1] }, cell: "called".into(), to: Some(recollect_graph::Stored::Value(json!("p"))) });
            ops.push(Op::set_formula(*id, "value", "p + 1"));
        }
    }
    (ops, ids)
}

fn number(g: &Graph, e: Entity) -> Value {
    g.get_value(e, "value").unwrap().unwrap()
}

#[test]
fn worked_out_in_stretches_is_the_same_as_in_one_go() {
    // Every sample, whole: the same ops, so the same handles, worked out both ways.
    for s in SAMPLES {
        let ops = ops_of(s);
        set_depth_limit(usize::MAX);
        let whole = everything(log_of(s.systems, ops.clone()).graph());
        set_depth_limit(5);
        let stretched = everything(log_of(s.systems, ops).graph());
        assert_eq!(whole, stretched, "{}", s.name);
    }

    // A chain much longer than a stretch, then changed at the bottom.
    set_depth_limit(20);
    let (ops, ids) = chain(600);
    let mut log = log_of(&[], ops);
    assert_eq!(number(log.graph(), ids[599]), json!(600.0));
    log.commit("t", "", vec![Op::set_value(ids[0], "value", json!(11.0))]).unwrap();
    assert_eq!(number(log.graph(), ids[599]), json!(610.0), "and an edit at the bottom reaches the top");
    assert_eq!(number(log.graph(), ids[300]), json!(311.0));

    // A plan as deep, with its checks.
    let depth = 400;
    let tasks: Vec<Entity> = (0..depth).map(|_| Entity::new()).collect();
    let mut ops: Vec<Op> = tasks.iter().flat_map(|t| [Op::Spawn { id: *t }, Op::add(*t, TASK)]).collect();
    ops.extend(tasks.windows(2).map(|w| Op::link(w[1], w[0], PART_OF)));
    set_depth_limit(usize::MAX);
    let mut reg = Registry::new();
    plan::install(&mut reg);
    let reg = Arc::new(reg);
    let mut whole = Log::new(reg.clone());
    whole.commit("t", "", ops.clone()).unwrap();
    let expected = (everything(whole.graph()), whole.violations());
    set_depth_limit(20);
    let mut stretched = Log::new(reg);
    stretched.commit("t", "", ops).unwrap();
    assert_eq!((everything(stretched.graph()), stretched.violations()), expected);
}

#[test]
fn a_loop_too_long_to_see_in_one_stretch_ends_in_an_error() {
    set_depth_limit(10);
    let (mut ops, ids) = chain(100);
    // Close it: the first reads the last.
    ops.retain(|op| !matches!(op, Op::Set { cell, .. } if cell == "value" && op.target() == recollect_graph::Target::Node(ids[0])));
    ops.push(Op::link(ids[0], ids[99], "label"));
    ops.push(Op::Set { on: recollect_graph::On::Edge { from: ids[0], to: ids[99] }, cell: "called".into(), to: Some(recollect_graph::Stored::Value(json!("p"))) });
    ops.push(Op::set_formula(ids[0], "value", "p + 1"));
    let log = log_of(&[], ops);
    let at = std::time::Instant::now();
    let got = log.graph().get_value(ids[50], "value");
    assert!(got.is_err(), "{got:?}");
    assert!(at.elapsed() < std::time::Duration::from_secs(30), "and it gives up in reasonable time: {:?}", at.elapsed());
}
