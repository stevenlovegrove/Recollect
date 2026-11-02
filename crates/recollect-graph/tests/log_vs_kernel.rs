//! The old kernel's log and invariants are an independent implementation of the same rules, with
//! a different compute model: it checks by walking a whole store on every commit, and this does it
//! with memoised rules. So random edits go through both, and they have to agree on what's refused,
//! why, and what the graph then is. After each step, this log also has to agree with itself: with
//! a new graph replaying its events, and with its files reopened.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use recollect_systems::plan;
use recollect_graph::{Log, Op, Registry, Runtime};
use recollect_kernel::log::Log as KLog;
use recollect_kernel::store::{Op as KOp, Store, Target as KTarget};
use serde_json::{Value, json};

#[path = "support/kernel.rs"]
mod kernel;
use kernel::Ids;

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.next(100) < pct
    }
}

fn random_op(rng: &mut Rng, names: &[String]) -> KOp {
    let pick = |rng: &mut Rng| names[rng.next(names.len())].clone();
    let (a, b) = (pick(rng), pick(rng));
    let node = KTarget::Entity(a.clone());
    let link = KTarget::Link(a, b.clone());
    let set = |t: KTarget, c: &str, v: Value| KOp::Set(t, c.into(), Some(v));
    match rng.next(30) {
        0..=3 => set(node, "task", json!({ "estimate": 1.0 })),
        4 => set(node, "goal", json!({})),
        5 => set(node, "unknown", json!({})),
        6 | 7 => set(node, "done", json!({})),
        8 => set(node, "dropped", json!({})),
        9 | 10 => set(node, "decision", json!({})),
        11 | 12 => set(node, "decision", json!({ "chosen": b })),
        13 => KOp::Set(node, ["done", "dropped", "unknown", "decision", "task"][rng.next(5)].into(), None),
        14..=17 => set(link, "part_of", json!({})),
        18..=20 => set(link, "alternative", json!({})),
        21 => set(link, "alternative", json!({ "ruled_out": {} })),
        22..=24 => set(link, "depends_on", json!({})),
        25 => set(link, "note", json!("hello")),
        26 => KOp::Remove(link),
        27 => KOp::Remove(node),
        // Things that are never right.
        28 => match rng.next(4) {
            0 => set(node, "task", json!("soon")),
            1 => set(node, "part_of", json!({})),
            2 => set(link, "done", json!({})),
            _ => set(link, "depends_on", json!({ "on": "whenever" })),
        },
        _ => set(node, "title", json!("just some data")),
    }
}

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    Arc::new(reg)
}

#[test]
fn random_edits_are_treated_the_same_as_by_the_old_kernel() {
    let (mut accepted, mut refused, mut undone, mut cells) = (0, 0, 0, 0);
    let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
    let mut seed = 0x9E3779B97F4A7C15u64;
    for round in 0..60 {
        seed = seed.wrapping_add(0x9E3779B97F4A7C15);
        let mut rng = Rng(seed | 1);
        let names: Vec<String> = (0..4 + rng.next(9)).map(|i| format!("n{i}")).collect();
        let mut ids = Ids(BTreeMap::new());
        let mut old = KLog::default();
        let mut new = Log::new(registry());
        let dir = tempfile::tempdir().unwrap();
        let on_disk = round % 6 == 0;
        let mut files = on_disk.then(|| Log::open(dir.path(), registry(), "ext").unwrap().0);

        for step in 0..120 {
            let undoing = rng.chance(8) && !old.events.is_empty();
            let kops: Vec<KOp> = if undoing {
                vec![]
            } else {
                (0..1 + rng.next(3)).map(|_| random_op(&mut rng, &names)).collect()
            };

            let (old_result, new_result, files_result) = if undoing {
                // Only the last event: undoing an older one has to reckon with what's happened
                // since, where the two deliberately differ (see docs/graph-core.md).
                let seq = old.events.len() - 1;
                undone += 1;
                (old.undo(seq, "t", "undo").map(|_| ()), new.undo(seq as u64, "t", "undo").map(|_| ()), files.as_mut().map(|f| f.undo(seq as u64, "t", "undo").map(|_| ())))
            } else {
                let mut state = old.store.clone();
                let mut ops = Vec::new();
                for op in &kops {
                    ops.extend(ids.op(op, &state));
                    let _ = state.apply(op);
                }
                let files_result = files.as_mut().map(|f| f.commit("t", "x", ops.clone()).map(|_| ()));
                (old.commit("t", "x", kops.clone()).map(|_| ()), new.commit("t", "x", ops).map(|_| ()), files_result)
            };

            let context = || format!("round {round} step {step}: {kops:?}");
            match (&old_result, &new_result) {
                (Ok(()), Ok(())) => accepted += 1,
                (Err(old_v), Err(new_e)) => {
                    refused += 1;
                    let old_rules: BTreeSet<&str> = old_v.iter().map(|v| v.rule).collect();
                    let new_rules: BTreeSet<&str> = new_e.violations().iter().map(|v| v.rule.as_str()).collect();
                    for r in &new_rules {
                        *by_rule.entry(r.to_string()).or_default() += 1;
                    }
                    if old_rules.contains("malformed_component") {
                        // A malformed value also stops other checks from reading it. The old
                        // kernel reads around it, so only this much has to agree.
                        assert!(new_rules.contains("malformed_component"), "{}: {old_rules:?} vs {new_rules:?}", context());
                    } else {
                        assert_eq!(old_rules, new_rules, "{}", context());
                    }
                }
                _ => panic!("{}: the old kernel said {old_result:?}, this says {new_result:?}", context()),
            }

            // The same graph.
            let saved = new.graph().save().unwrap();
            assert_eq!(saved, ids.store(&old.store), "{}", context());

            if let (Some(f), Some(r)) = (&files, &files_result) {
                assert_eq!(r.is_ok(), new_result.is_ok(), "{}", context());
                assert_eq!(f.graph().save().unwrap(), saved, "on disk: {}", context());
            }

            // Memos are invisible: every cell reads as it would in a new graph that replayed it,
            // including after refusals, which change and then restore the graph.
            if step % 5 == 0 {
                let fresh = match new.last_seq() {
                    Some(s) => new.state_at(s),
                    None => recollect_graph::Graph::with_registry(registry()),
                };
                assert_eq!(fresh.save().unwrap(), saved, "{}", context());
                for e in new.graph().entities() {
                    for c in new.graph().components_of(e) {
                        cells += 1;
                        match (new.graph().get_runtime(e, &c), fresh.get_runtime(e, &c)) {
                            (Err(recollect_graph::Error::Cycle(_)), Err(recollect_graph::Error::Cycle(_))) => {}
                            (a, b) => assert_eq!(a, b, "{c} of {e}: {}", context()),
                        }
                    }
                }
            }
        }

        if let Some(f) = files.take() {
            let saved = f.graph().save().unwrap();
            let events = f.events().to_vec();
            drop(f);
            let (reopened, problems) = Log::open(dir.path(), registry(), "ext").unwrap();
            assert_eq!(problems, vec![], "round {round}");
            assert_eq!(reopened.graph().save().unwrap(), saved);
            assert_eq!(reopened.events(), events);
            let (plain, _) = recollect_graph::Graph::load_dir(dir.path(), registry()).unwrap();
            assert_eq!(plain.save().unwrap(), saved, "the files are the same graph");
        }
    }
    println!("{accepted} accepted, {refused} refused, {undone} undos, {cells} cells compared with a replay");
    println!("refused by: {by_rule:?}");
    assert!(accepted > 2000 && refused > 1000, "{accepted} accepted, {refused} refused");
    for rule in ["no_deadlock", "chosen_must_be_a_live_option", "one_decomposition_per_link", "unknown_cannot_be_done", "unknown_cannot_be_chosen", "unknown_has_no_parts", "link_target_lacks_component", "malformed_component", "component_on_wrong_target", "no_self_links", "op_failed"] {
        assert!(by_rule.get(rule).is_some_and(|n| *n > 3), "{rule} barely ran: {by_rule:?}");
    }
}

/// Any written-down state can be reconciled to, exactly, and undone exactly.
#[test]
fn reconciling_any_external_state_records_it_exactly() {
    let mut rng = Rng(0xDEADBEEFCAFE);
    let mut compared = 0;
    for _ in 0..200 {
        let names: Vec<String> = (0..6).map(|i| format!("n{i}")).collect();
        let mut ids = Ids(BTreeMap::new());
        let (mut a, mut b) = (Store::default(), Store::default());
        for store in [&mut a, &mut b] {
            for _ in 0..25 {
                let op = random_op(&mut rng, &names);
                let _ = store.apply(&op);
            }
            // The old kernel keeps a link with nothing on it. Here, a link is its kinds.
            store.links.retain(|_, c| !c.is_empty());
        }
        let (from, to) = (ids.store(&a), ids.store(&b));

        let mut log = Log::new(registry());
        log.reconcile(&from, "start").unwrap();
        assert_eq!(log.graph().save().unwrap(), from);
        let before = log.events().len();
        log.reconcile(&to, "edit").unwrap();
        assert_eq!(log.graph().save().unwrap(), to, "the graph matches what it was given");
        if from != to {
            assert_eq!(log.events().len(), before + 1, "as one event");
            let seq = log.last_seq().unwrap();
            assert_eq!(log.state_at(seq).save().unwrap(), to, "replay agrees");
            let undo_ops = log.events()[seq as usize].inverse.clone();
            let mut replay = log.state_at(seq);
            for op in &undo_ops {
                recollect_graph_apply(&mut replay, op);
            }
            assert_eq!(replay.save().unwrap(), from, "and its inverse is exact");
            compared += 1;
        }
    }
    assert!(compared > 150, "{compared}");
}

fn recollect_graph_apply(g: &mut recollect_graph::Graph, op: &Op) {
    let saved_rule = |s: &recollect_graph::Stored| match s {
        recollect_graph::Stored::Value(v) => v.clone(),
        recollect_graph::Stored::Rule(r) => panic!("these graphs have no rules: {r:?}"),
    };
    match op {
        Op::Spawn { id } => {
            g.adopt(*id);
        }
        Op::Despawn { id } => assert!(g.despawn(*id), "{id} still has links"),
        Op::Set { id, component, to } => {
            g.adopt(*id);
            match to {
                Some(s) => {
                    g.set_runtime(*id, component, saved_rule(s));
                }
                None => {
                    g.remove_named(*id, component);
                }
            }
        }
        Op::Link { from, to, kind, value } => {
            match value {
                Some(v) => g.link_runtime(*from, *to, Runtime::new(kind, v.clone())),
                None => g.unlink_named(*from, *to, kind),
            };
        }
        Op::Default { .. } => panic!("these graphs have no defaults"),
    }
}
