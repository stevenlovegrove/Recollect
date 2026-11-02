//! Is the kernel really exact? Random graphs are checked against a separate evaluator that
//! walks every world directly from the spec's definitions, sharing no code with the formulas.

use std::collections::BTreeMap;

use recollect_kernel::formula::TRUE;
use recollect_kernel::log::{Log, choose, set};
use recollect_kernel::store::*;
use recollect_kernel::systems::*;
use serde_json::json;

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

type World<'a> = BTreeMap<&'a str, &'a str>;

fn selects(s: &Store, w: &World, dec: &str, opt: &str) -> bool {
    match s.chosen(dec) {
        Some(c) => c == opt,
        None => w.get(dec) == Some(&opt),
    }
}

/// Spec: need(n) = false if dropped; true if no decomposition parents; else OR over parents.
fn need(s: &Store, w: &World, id: &str) -> bool {
    if !s.live(id) {
        return false;
    }
    let ups: Vec<_> = s.links.iter().filter(|((f, _), c)| f == id && (c.contains_key(PART_OF) || c.contains_key(ALTERNATIVE))).collect();
    if ups.is_empty() {
        return true;
    }
    ups.iter().any(|((f, t), c)| {
        if c.contains_key(PART_OF) {
            need(s, w, t)
        } else {
            c[ALTERNATIVE].get("ruled_out").is_none() && need(s, w, t) && selects(s, w, t, f)
        }
    })
}

/// Spec: done(n) = stated ∨ all_required_done(n); dropped is neither.
fn done(s: &Store, w: &World, id: &str) -> bool {
    s.live(id) && (s.has(id, DONE) || all_required(s, w, id))
}

fn all_required(s: &Store, w: &World, id: &str) -> bool {
    let parts = s.children(id, PART_OF);
    let is_decision = s.has(id, DECISION);
    if parts.is_empty() && !is_decision {
        return !["task", "goal", "question", "risk", "unknown"].iter().any(|c| s.has(id, c));
    }
    parts.iter().all(|(c, _)| done(s, w, c)) && (!is_decision || s.options(id).iter().any(|(o, _)| selects(s, w, id, o) && done(s, w, o)))
}

/// Spec: blocked(n) = some own or ancestor dependency waits. Default: needed and not done.
/// `{on: decided}`: the target is an open decision. Dropped targets don't wait.
fn blocked(s: &Store, w: &World, id: &str) -> bool {
    let mut scope = vec![id.to_string()];
    let mut i = 0;
    while i < scope.len() {
        for p in s.parents(&scope[i]) {
            if !scope.iter().any(|x| x == p) {
                scope.push(p.to_string());
            }
        }
        i += 1;
    }
    scope.iter().any(|from| {
        s.links.iter().filter(|((f, _), c)| f == from && c.contains_key(DEPENDS_ON)).any(|((_, t), c)| {
            s.live(t)
                && if c[DEPENDS_ON].get("on").is_some() {
                    s.has(t, DECISION) && s.chosen(t).is_none()
                } else {
                    need(s, w, t) && !done(s, w, t)
                }
        })
    })
}

/// Spec: a node's scope is the nearest node every path from the top must pass through. Checked
/// the slow way, by asking which nodes can't be reached from the top without passing through it.
fn reachable_avoiding(s: &Store, id: &str, avoid: &str) -> bool {
    if id == avoid {
        return false;
    }
    let parents = s.parents(id);
    parents.is_empty() || parents.iter().any(|p| reachable_avoiding(s, p, avoid))
}

fn scope_by_definition(s: &Store, id: &str) -> Option<String> {
    let dominators: Vec<&String> = s.entities.keys().filter(|a| a.as_str() != id && !reachable_avoiding(s, id, a)).collect();
    // The nearest is the one that all the others also dominate.
    dominators.iter().find(|a| dominators.iter().all(|b| *a == b || !reachable_avoiding(s, a, b))).map(|a| a.to_string())
}

fn random_graph(rng: &mut Rng) -> Log {
    let n = 6 + rng.next(14);
    let ids: Vec<String> = (0..n).map(|i| format!("n{i:02}")).collect();
    let mut ops = Vec::new();
    let mut decisions = Vec::new();
    for id in &ids {
        ops.push(set(e(id), "title", json!(id)));
        if rng.chance(25) && decisions.len() < 5 {
            ops.push(set(e(id), DECISION, json!({})));
            decisions.push(id.clone());
        } else if rng.chance(50) {
            ops.push(set(e(id), TASK, json!({})));
        }
        if rng.chance(25) {
            ops.push(set(e(id), DONE, json!({})));
        } else if rng.chance(12) {
            ops.push(set(e(id), UNKNOWN, json!({})));
        }
        if rng.chance(8) {
            ops.push(set(e(id), DROPPED, json!({})));
        }
    }
    // Links only point at earlier nodes, so decomposition is acyclic by construction.
    for i in 1..n {
        for _ in 0..(1 + rng.next(2)) {
            let parent = &ids[rng.next(i)];
            if decisions.contains(parent) && rng.chance(70) {
                let v = if rng.chance(15) { json!({ "ruled_out": {} }) } else { json!({}) };
                ops.push(set(l(&ids[i], parent), ALTERNATIVE, v));
            } else {
                ops.push(set(l(&ids[i], parent), PART_OF, json!({})));
            }
        }
    }
    // Dependencies point only at later nodes, against the decomposition, which keeps most acyclic.
    // Any that would deadlock are skipped below.
    let mut deps = Vec::new();
    for _ in 0..(n / 3) {
        let (a, b) = (rng.next(n), rng.next(n));
        if a != b {
            let v = if rng.chance(25) { json!({ "on": "decided" }) } else { json!({}) };
            deps.push(set(l(&ids[a], &ids[b]), DEPENDS_ON, v));
        }
    }
    // Set on a link twice means last wins, which can leave one link both part_of and
    // alternative. Keep only the first decomposition per link.
    let mut seen = std::collections::BTreeSet::new();
    ops.retain(|op| match op {
        Op::Set(Target::Link(f, t), _, _) => seen.insert((f.clone(), t.clone())),
        _ => true,
    });
    // A gap can't have parts, so nothing may be part_of one.
    let gaps: std::collections::BTreeSet<String> = ops.iter().filter_map(|op| match op {
        Op::Set(Target::Entity(id), c, _) if c == UNKNOWN => Some(id.clone()),
        _ => None,
    }).collect();
    ops.retain(|op| !matches!(op, Op::Set(Target::Link(_, t), c, _) if c == PART_OF && gaps.contains(t)));
    let mut log = Log::default();
    log.commit("gen", "random", ops).expect("generator makes valid graphs");
    for dep in deps {
        // Refused if it would deadlock or collide: that's fine, it just isn't added.
        let _ = log.commit("gen", "dep", vec![dep]);
    }
    for d in &decisions {
        if rng.chance(30) {
            let pick = log.store.options(d).iter().map(|(o, _)| o.to_string()).find(|o| !log.store.has(o, UNKNOWN));
            if let Some(o) = pick {
                log.commit("gen", "decide", vec![choose(d, &o)]).expect("valid choice");
            }
        }
    }
    log
}

#[test]
fn formulas_agree_with_brute_force_on_random_graphs() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut worlds_checked = 0usize;
    let mut with_open_decisions = 0usize;
    let (mut blocked_true, mut gap_selected) = (0usize, 0usize);
    for _ in 0..400 {
        let log = random_graph(&mut rng);
        let s = &log.store;
        let d = compute(s);
        if !d.mdd.vars.is_empty() {
            with_open_decisions += 1;
        }
        // Every assignment of every open decision.
        let mut world: Vec<usize> = vec![0; d.mdd.vars.len()];
        loop {
            let w: World = d.mdd.vars.iter().zip(&world).map(|(v, i)| (v.name.as_str(), v.options[*i].as_str())).collect();
            for id in s.entities.keys() {
                assert_eq!(d.mdd.eval(d.get("need", id), &w), need(s, &w, id), "need({id}) in {w:?}");
                assert_eq!(d.mdd.eval(d.get("done", id), &w), done(s, &w, id), "done({id}) in {w:?}");
                assert_eq!(d.mdd.eval(d.get("blocked", id), &w), blocked(s, &w, id), "blocked({id}) in {w:?}");
                blocked_true += usize::from(blocked(s, &w, id));
                if s.has(id, UNKNOWN) && need(s, &w, id) {
                    gap_selected += 1;
                }
                worlds_checked += 1;
            }
            let mut i = 0;
            loop {
                if i == world.len() {
                    break;
                }
                world[i] += 1;
                if world[i] < d.mdd.vars[i].options.len() {
                    break;
                }
                world[i] = 0;
                i += 1;
            }
            if i == world.len() {
                break;
            }
        }
    }
    println!("{worlds_checked} node-world pairs across 400 graphs, {with_open_decisions} with open decisions, {blocked_true} blocked, {gap_selected} where a gap is needed");
    assert!(blocked_true > 100 && gap_selected > 100, "the generator should exercise dependencies and gaps");
    assert!(with_open_decisions > 100, "the generator should actually exercise decisions");
}

#[test]
fn probability_is_the_weighted_count_of_worlds() {
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    for _ in 0..100 {
        let log = random_graph(&mut rng);
        let s = &log.store;
        let d = compute(s);
        for id in s.entities.keys() {
            let f = d.get("need", id);
            let mut world: Vec<usize> = vec![0; d.mdd.vars.len()];
            let mut total = 0.0;
            loop {
                let w: World = d.mdd.vars.iter().zip(&world).map(|(v, i)| (v.name.as_str(), v.options[*i].as_str())).collect();
                if d.mdd.eval(f, &w) {
                    total += d.mdd.vars.iter().zip(&world).map(|(v, i)| v.weights[*i]).product::<f64>();
                }
                let mut i = 0;
                loop {
                    if i == world.len() {
                        break;
                    }
                    world[i] += 1;
                    if world[i] < d.mdd.vars[i].options.len() {
                        break;
                    }
                    world[i] = 0;
                    i += 1;
                }
                if i == world.len() {
                    break;
                }
            }
            assert!((d.mdd.prob(f) - total).abs() < 1e-9, "{id}: {} vs {total}", d.mdd.prob(f));
        }
    }
}

#[test]
fn scope_is_the_immediate_dominator() {
    let mut rng = Rng(0x123456789ABCDEF);
    let mut multi_parent = 0;
    for _ in 0..300 {
        let log = random_graph(&mut rng);
        let s = &log.store;
        let d = compute(s);
        for id in s.entities.keys() {
            if s.parents(id).len() > 1 {
                multi_parent += 1;
            }
            let got = d.of::<recollect_kernel::scope::Scope>(id).unwrap().clone();
            assert_eq!(got, scope_by_definition(s, id), "scope({id})");
        }
    }
    println!("{multi_parent} nodes with several parents");
    assert!(multi_parent > 100, "the generator should actually exercise sharing");
}

#[test]
fn collapsing_keeps_every_node_and_every_crossing_link() {
    use recollect_kernel::scope::ViewGraph;
    let mut rng = Rng(0xFEEDFACE12345678);
    for _ in 0..200 {
        let log = random_graph(&mut rng);
        let s = &log.store;
        let d = compute(s);
        let full = ViewGraph::of(s);
        // Expanding everything changes nothing.
        let all: std::collections::BTreeSet<String> = s.entities.keys().cloned().collect();
        assert_eq!(full.collapse(&d, &all), full);
        // Fold a random selection: every node is either visible or counted once.
        let some = all.iter().filter(|_| rng.chance(50)).cloned().collect();
        let v = full.collapse(&d, &some);
        let shown: usize = v.nodes.values().map(|n| 1 + n.hidden).sum();
        assert_eq!(shown, s.entities.len());
        // Folding twice is the same as folding once.
        assert_eq!(v.collapse(&d, &some), v);
        // An edge in the view stands for at least one real link.
        assert!(v.edges.values().map(|e| e.links).sum::<usize>() <= s.links.len());
    }
}

#[test]
fn undo_restores_the_store_exactly() {
    let mut rng = Rng(0xABCDEF0123456789);
    for _ in 0..200 {
        let mut log = random_graph(&mut rng);
        for seq in (0..log.events.len()).rev() {
            let before = log.state_at(seq.saturating_sub(1));
            log.undo(seq, "t", "undo").unwrap();
            if seq > 0 {
                assert_eq!(log.store, before, "undoing event {seq}");
            } else {
                assert_eq!(log.store, Store::default(), "undoing everything leaves nothing");
            }
        }
    }
}

#[test]
fn reconciling_any_external_state_records_it_exactly() {
    let mut rng = Rng(0x0F1E2D3C4B5A6978);
    for _ in 0..200 {
        let mut log = random_graph(&mut rng);
        let start = log.store.clone();
        let disk = random_graph(&mut rng).store;
        log.reconcile(&disk, "hand");
        assert_eq!(log.store, disk);
        let seq = log.events.len() - 1;
        assert_eq!(log.state_at(seq), disk, "replay agrees");
        log.undo(seq, "t", "undo").unwrap();
        assert_eq!(log.store, start);
    }
}

/// The classic worst case for decision diagrams: (a0∧b0) ∨ (a1∧b1) ∨ … with all the a's
/// ordered before the b's, so the diagram has to remember every a. It arises naturally as one
/// task shared under many pairs of nested decisions. Not run by default; it's a measurement.
#[test]
#[ignore]
fn worst_case_scaling() {
    for pairs in [4, 8, 12, 14, 16, 18, 20] {
        let mut ops = vec![set(e("shared"), TASK, json!({}))];
        for i in 0..pairs {
            let (a, b) = (format!("a{i:02}"), format!("b{i:02}"));
            for dec in [&a, &b] {
                ops.push(set(e(dec), DECISION, json!({})));
                for o in ["x", "y"] {
                    let opt = format!("{dec}{o}");
                    ops.push(set(e(&opt), "title", json!(opt)));
                    ops.push(set(l(&opt, dec), ALTERNATIVE, json!({})));
                }
            }
            ops.push(set(l(&b, &format!("{a}x")), PART_OF, json!({})));
            ops.push(set(l("shared", &format!("{b}x")), PART_OF, json!({})));
        }
        let mut log = Log::default();
        log.commit("gen", "worst case", ops).unwrap();
        let start = std::time::Instant::now();
        let d = compute(&log.store);
        println!(
            "{pairs:2} pairs ({:2} decisions, {:3} entities): {:?}, P(need) = {:.3}",
            d.mdd.vars.len(),
            log.store.entities.len(),
            start.elapsed(),
            d.prob("need", "shared")
        );
        assert_ne!(d.get("need", "shared"), TRUE);
    }
}
