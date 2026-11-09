//! What the planning rules cost. These are measurements rather than checks, so they don't run by
//! default: `cargo test --release -p recollect-systems --test plan_cost -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Instant;

use recollect_graph::{Entity, Graph, Log, Name, Op, Registry};
use recollect_systems::plan::{self, *};

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    Arc::new(reg)
}

fn read_everything(g: &Graph) {
    for e in g.entities() {
        g.get::<Need>(e).unwrap();
        g.get::<Done>(e).unwrap();
        g.get::<Blocked>(e).unwrap();
        g.get::<Actionable>(e).unwrap();
        g.get::<Scope>(e).unwrap();
    }
}

/// A new node, with these components.
fn spawn(ops: &mut Vec<Op>, components: &[&str]) -> Entity {
    let e = Entity::new();
    ops.push(Op::Spawn { id: e });
    ops.extend(components.iter().map(|c| Op::add(e, c)));
    e
}

/// A new node with a name.
fn named(ops: &mut Vec<Op>, name: &str) -> Entity {
    let e = spawn(ops, &[]);
    ops.push(Op::set(e, Name(name.to_string())));
    e
}

/// The classic worst case for decision diagrams: (a0∧b0) ∨ (a1∧b1) ∨ … with all the a's ordered
/// before the b's, so the diagram has to remember every a. It arises as one task shared under many
/// pairs of nested decisions.
#[test]
#[ignore]
fn one_task_shared_under_nested_decisions() {
    for pairs in [4, 8, 12, 14, 16, 18, 20] {
        let mut ops = Vec::new();
        let shared = spawn(&mut ops, &[TASK]);
        let mut levels = Vec::new();
        for _ in 0..pairs {
            let (a, b) = (spawn(&mut ops, &[DECISION]), spawn(&mut ops, &[DECISION]));
            let mut x = [None, None];
            for dec in [a, b] {
                let (ox, oy) = (named(&mut ops, "x"), named(&mut ops, "y"));
                ops.extend([Op::link(ox, dec, ALTERNATIVE), Op::link(oy, dec, ALTERNATIVE)]);
                x[usize::from(dec == b)] = Some(ox);
            }
            levels.push((a, b, x[0].unwrap(), x[1].unwrap()));
        }
        for (_, b, ax, bx) in &levels {
            ops.extend([Op::link(*b, *ax, PART_OF), Op::link(shared, *bx, PART_OF)]);
        }
        let mut log = Log::new(registry());
        log.commit("t", "worst case", ops).unwrap();
        // Outer decisions first, as the kernel orders them, for a fair comparison.
        for (a, _, _, _) in &levels {
            log.graph().get::<Choice>(*a).unwrap();
        }
        let start = Instant::now();
        let need = log.graph().get::<Need>(shared).unwrap().unwrap().0;
        println!("{pairs:2} pairs ({:2} decisions): {:?}, P(need) = {:.3}", pairs * 2, start.elapsed(), need.prob());
    }
}

#[test]
#[ignore]
fn many_independent_decisions() {
    let mut ops = Vec::new();
    let root = named(&mut ops, "root");
    for _ in 0..40 {
        let (dec, a, b) = (spawn(&mut ops, &[DECISION]), spawn(&mut ops, &[TASK]), spawn(&mut ops, &[TASK]));
        ops.extend([Op::link(dec, root, PART_OF), Op::link(a, dec, ALTERNATIVE), Op::link(b, dec, ALTERNATIVE)]);
        for _ in 0..20 {
            let t = spawn(&mut ops, &[TASK]);
            ops.push(Op::link(t, a, PART_OF));
        }
    }
    let mut log = Log::new(registry());
    log.commit("t", "import", ops).unwrap();
    let start = Instant::now();
    read_everything(log.graph());
    println!("{} nodes, 2^40 worlds: every cell read in {:?}", log.graph().entities().count(), start.elapsed());
}

/// The same shape as the log's measurement, with something reading the plan after each edit, as a
/// screen showing it would.
#[test]
#[ignore]
fn an_edit_then_reading_the_whole_plan() {
    for tasks in [1_000usize, 10_000] {
        let phases = tasks / 10;
        let mut ops = Vec::new();
        let (mut decisions, mut leaves) = (Vec::new(), Vec::new());
        let mut last_phase = None;
        for p in 0..phases {
            let phase = named(&mut ops, &format!("phase {p}"));
            if let Some(last) = last_phase {
                ops.push(Op::link(phase, last, DEPENDS_ON));
            }
            last_phase = Some(phase);
            // Every tenth phase holds a decision between two approaches, and the rest is plain.
            let (under_a, under_b) = if p % 10 == 0 {
                let dec = spawn(&mut ops, &[DECISION]);
                let (a, b) = (named(&mut ops, "a"), named(&mut ops, "b"));
                ops.extend([Op::link(dec, phase, PART_OF), Op::link(a, dec, ALTERNATIVE), Op::link(b, dec, ALTERNATIVE)]);
                decisions.push((dec, a));
                (a, b)
            } else {
                (phase, phase)
            };
            for t in 0..10 {
                let leaf = spawn(&mut ops, &[TASK]);
                ops.push(Op::link(leaf, if t % 2 == 0 { under_a } else { under_b }, PART_OF));
                leaves.push(leaf);
            }
        }

        let mut log = Log::new(registry());
        let start = Instant::now();
        log.commit("t", "import", ops).unwrap();
        let import = start.elapsed();
        let start = Instant::now();
        read_everything(log.graph());
        let first = start.elapsed();

        let edits = 50;
        let (mut commit, mut read) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for i in 0..edits {
            let leaf = leaves[i * leaves.len() / edits];
            let start = Instant::now();
            log.commit("t", "finished", vec![Op::set(leaf, Done::yes())]).unwrap();
            commit += start.elapsed();
            let start = Instant::now();
            read_everything(log.graph());
            read += start.elapsed();
        }
        let start = Instant::now();
        let (d, o) = decisions[decisions.len() / 2];
        log.commit("t", "decided", vec![choose(d, o)]).unwrap();
        let decide_commit = start.elapsed();
        let start = Instant::now();
        read_everything(log.graph());
        let decide_read = start.elapsed();
        println!(
            "{tasks} tasks, {} decisions: import {import:?}, first read of everything {first:?}; each edit {:?} to commit and {:?} to reread; deciding {decide_commit:?} and {decide_read:?}",
            decisions.len(),
            commit / edits as u32,
            read / edits as u32
        );
    }
}
