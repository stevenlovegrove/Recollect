//! What dates cost. These are measurements rather than checks, so they don't run by default:
//! `cargo test --release -p recollect-systems --test schedule_cost -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Instant;

use recollect_graph::{Entity, Graph, Log, Name, Op, Registry};
use recollect_systems::plan::{self, *};
use recollect_systems::schedule;

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    schedule::install(&mut reg);
    Arc::new(reg)
}

fn read_dates(g: &Graph) {
    for e in g.entities() {
        for c in ["start", "finish", "slack"] {
            g.get_value(e, c).unwrap();
        }
    }
}

/// A new node with a name.
fn named(ops: &mut Vec<Op>, name: &str) -> Entity {
    let e = Entity::new();
    ops.extend([Op::Spawn { id: e }, Op::set(e, Name(name.to_string()))]);
    e
}

/// Phases in a chain of ten tasks each. `open` of them hold a decision between two approaches, whose
/// tasks take different lengths of time if `differ`, so dates have up to 2^open values each.
fn plan(tasks: usize, open: usize, differ: bool) -> (Vec<Op>, Vec<Entity>) {
    let phases = tasks / 10;
    let mut ops = Vec::new();
    let mut leaves = Vec::new();
    let mut last = None;
    for p in 0..phases {
        let phase = named(&mut ops, &format!("phase {p}"));
        if let Some(last) = last {
            ops.push(Op::link(phase, last, DEPENDS_ON));
        }
        last = Some(phase);
        let (a, b) = if p < open {
            let dec = Entity::new();
            ops.extend([Op::Spawn { id: dec }, Op::add(dec, DECISION)]);
            let (a, b) = (named(&mut ops, "a"), named(&mut ops, "b"));
            ops.extend([Op::link(dec, phase, PART_OF), Op::link(a, dec, ALTERNATIVE), Op::link(b, dec, ALTERNATIVE)]);
            (a, b)
        } else {
            (phase, phase)
        };
        for t in 0..10 {
            let leaf = Entity::new();
            let under_a = t % 2 == 0;
            let days = if differ && under_a { 2.0 + p as f64 } else { 1.0 };
            ops.extend([Op::Spawn { id: leaf }, Op::add(leaf, TASK), Op::set(leaf, Estimate::days(days)), Op::link(leaf, if under_a { a } else { b }, PART_OF)]);
            leaves.push(leaf);
        }
    }
    (ops, leaves)
}

#[test]
#[ignore]
fn an_edit_then_reading_every_date() {
    for (tasks, open, differ) in [(1_000, 0, false), (3_000, 0, false), (1_000, 10, false), (1_000, 8, true)] {
        let (ops, leaves) = plan(tasks, open, differ);
        let mut log = Log::new(registry());
        log.commit("t", "import", ops).unwrap();
        let start = Instant::now();
        read_dates(log.graph());
        let first = start.elapsed();
        let edits = 5;
        let (mut commit, mut read) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for i in 0..edits {
            let start = Instant::now();
            log.commit("t", "longer", vec![Op::set(leaves[i * leaves.len() / edits], Estimate::days(50.0))]).unwrap();
            commit += start.elapsed();
            let start = Instant::now();
            read_dates(log.graph());
            read += start.elapsed();
        }
        println!("{tasks} tasks, {open} decisions, differ {differ}: first read {first:?}; each edit {:?} to commit and {:?} to reread", commit / edits as u32, read / edits as u32);
    }
}
